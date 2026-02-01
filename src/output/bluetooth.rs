use anyhow::{Result, anyhow};
use async_trait::async_trait;
use bluer::agent::Agent;
use bluer::l2cap::{SocketAddr, StreamListener};
use bluer::rfcomm::{Profile, ProfileHandle, Role};
use bluer::{Adapter, AdapterEvent, Address, AddressType};
use libc::seccomp_data;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use usb_gadget::function::hid::Hid;
use uuid::Uuid;

use super::{
    HidLedReader, HidReportSender, InputReport, KeyboardHidDevice, KeyboardModifiers, LedState,
    MouseButtons, MouseHidDevice,
};

const PSM_HID_CONTROL: u16 = 0x0011; // 17
const PSM_HID_INTERRUPT: u16 = 0x0013; // 19

const KEYBOARD_SDP_RECORD: &str = r#"
<?xml version="1.0" encoding="UTF-8" ?>
<record>
  <attribute id="0x0001">
    <sequence>
      <uuid value="0x1124" />
    </sequence>
  </attribute>
  <attribute id="0x0004">
    <sequence>
      <sequence>
        <uuid value="0x0100" />
        <uint16 value="0x0011" />
      </sequence>
      <sequence>
        <uuid value="0x0011" />
      </sequence>
    </sequence>
  </attribute>
  <attribute id="0x0005">
    <sequence>
      <uuid value="0x1002" />
    </sequence>
  </attribute>
  <attribute id="0x0006">
    <sequence>
      <uint16 value="0x656e" />
      <uint16 value="0x006a" />
      <uint16 value="0x0100" />
    </sequence>
  </attribute>
  <attribute id="0x0009">
    <sequence>
      <sequence>
        <uuid value="0x1124" />
        <uint16 value="0x0100" />
      </sequence>
    </sequence>
  </attribute>
  <attribute id="0x000d">
    <sequence>
      <sequence>
        <sequence>
          <uuid value="0x0100" />
          <uint16 value="0x0013" />
        </sequence>
        <sequence>
          <uuid value="0x0011" />
        </sequence>
      </sequence>
    </sequence>
  </attribute>
  <attribute id="0x0100">
    <text value="Virtual Keyboard" />
  </attribute>
  <attribute id="0x0101">
    <text value="USB > BT Keyboard" />
  </attribute>
  <attribute id="0x0102">
    <text value="Virtual Input" />
  </attribute>
  <attribute id="0x0200">
    <uint16 value="0x0100" />
  </attribute>
  <attribute id="0x0201">
    <uint16 value="0x0111" />
  </attribute>
    <attribute id="0x0202">
    <uint8 value="0xC0" />
    </attribute>
  <attribute id="0x0203">
    <uint8 value="0x21" />
  </attribute>
  <attribute id="0x0204">
    <boolean value="true" />  <!-- NormallyConnectable = false 表示需要保持连接 -->
  </attribute>
  <attribute id="0x0205">
    <boolean value="true" />
  </attribute>
  <attribute id="0x0206">
    <sequence>
      <sequence>
        <uint8 value="0x22" />
        <text encoding="hex" value="05010906a1018501050719e029e71500250175019508810295017508810195057501050819012905910295017503910195067508150025650507190029658100c005010902a10185020901a100050919012903150025019503750181029505750181010501093009311581257f750895028106c0c0" />
      </sequence>
    </sequence>
  </attribute>
  <attribute id="0x0207">
    <sequence>
      <sequence>
        <uint16 value="0x0409" />
        <uint16 value="0x0100" />
      </sequence>
    </sequence>
  </attribute>
  <attribute id="0x0209">
  <uint16 value="0x0012" />
</attribute>
<attribute id="0x020A">
  <uint16 value="0x0640" />
</attribute>
</record>
"#;

/// 蓝牙 HID 键盘设备
pub struct BluetoothKeyboardHidDevice {
    adapter: Arc<bluer::Adapter>,
    current_keys: [u8; 6],
    current_modifiers: KeyboardModifiers,
    // 使用 bluer 提供的 Stream 类型
    control_socket: Arc<Mutex<Option<bluer::l2cap::Stream>>>,
    interrupt_socket: Arc<Mutex<Option<bluer::l2cap::Stream>>>,
    session: bluer::Session,
    _agent_handle: Arc<bluer::agent::AgentHandle>,
}

pub struct BluetoothMouseHidDevice {
    adapter: Arc<bluer::Adapter>,
    current_buttons: MouseButtons,
    // 同理修改鼠标
    interrupt_socket: Arc<Mutex<Option<bluer::l2cap::Stream>>>,
    session: bluer::Session,
    _agent_handle: Arc<bluer::agent::AgentHandle>,
}

/// 创建并初始化蓝牙 HID 设备
pub async fn build_bluetooth_hid_device() -> Result<(
    BluetoothKeyboardHidDevice,
    BluetoothMouseHidDevice,
    bluer::Session,
)> {
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;

    // 设置适配器为可发现和可连接
    adapter.set_powered(true).await?;
    adapter.set_discoverable(true).await?;
    adapter.set_pairable(true).await?;

    // 设置设备名称
    adapter
        .set_alias("Virtual Keyboard Mouse".to_string())
        .await?;

    let agent = Agent {
        request_default: true, // 必须为 true，抢占系统默认代理
        request_passkey: Some(Box::new(|req| {
            Box::pin(async move {
                println!("iPad 请求输入 Passkey");
                Ok(123456) // 测试用固定 PIN
            })
        })),

        display_passkey: Some(Box::new(|req| {
            Box::pin(async move {
                println!(
                    "显示 Passkey 给用户: {} (entered {})",
                    req.passkey, req.entered
                );
                Ok(())
            })
        })),

        request_confirmation: Some(Box::new(|req| {
            Box::pin(async move {
                println!("确认配对: {}", req.passkey);
                Ok(())
            })
        })),
        authorize_service: Some(Box::new(|req| {
            Box::pin(async move {
                println!("授权服务请求: 设备 {} 访问服务 {}", req.device, req.service);
                Ok(()) // 返回 Ok(()) 表示同意访问
            })
        })),

        // 增加通用授权请求
        request_authorization: Some(Box::new(|req| {
            Box::pin(async move {
                println!("自动授权配对请求: {}", req.device);
                Ok(())
            })
        })),

        ..Default::default()
    };

    // 注册 Agent
    let _agent_handle = session.register_agent(agent).await?;
    println!("Bluetooth Agent 已注册并设置为默认");

    log::info!("蓝牙适配器已配置: {}", adapter.name());
    log::info!("适配器地址: {}", adapter.address().await?);

    let control_socket = Arc::new(Mutex::new(None));
    let interrupt_socket = Arc::new(Mutex::new(None));

    let shared_handle = Arc::new(_agent_handle);
    let shared_adpter = Arc::new(adapter);

    let keyboard = BluetoothKeyboardHidDevice {
        adapter: Arc::clone(&shared_adpter),
        current_keys: [0u8; 6],
        current_modifiers: KeyboardModifiers::default(),
        control_socket: Arc::clone(&control_socket),
        interrupt_socket: Arc::clone(&interrupt_socket),
        session: session.clone(),
        _agent_handle: Arc::clone(&shared_handle),
    };

    let mouse = BluetoothMouseHidDevice {
        adapter: Arc::clone(&shared_adpter),
        current_buttons: MouseButtons::default(),
        interrupt_socket: Arc::clone(&interrupt_socket),
        session: session.clone(),
        _agent_handle: Arc::clone(&shared_handle),
    };

    Ok((keyboard, mouse, session))
}

/// 启动 L2CAP 监听并注册服务
pub async fn run_server(
    keyboard: &BluetoothKeyboardHidDevice,
    session: &bluer::Session,
) -> Result<()> {
    // 1. 获取 Session
    let session = session.clone();

    // 2. 构造 Profile
    // 这里的 UUID 使用 HID 标准服务 UUID
    let hid_uuid = Uuid::parse_str("00001124-0000-1000-8000-00805f9b34fb")?;

    let profile = Profile {
        uuid: hid_uuid,
        name: Some("Virtual Keyboard".to_string()),
        service_record: Some(KEYBOARD_SDP_RECORD.to_string()),
        // psm: Some(17),
        role: Some(Role::Server),

        require_authentication: Some(false),
        require_authorization: Some(false),

        ..Default::default()
    };

    // 3. 注册 Profile
    // 这一步替代了之前的 add_sdp_record
    // 只要 _profile_handle 不被 drop，SDP 记录就一直有效
    let _profile_handle = session.register_profile(profile).await?;
    println!("HID Profile 已通过 ProfileManager1 注册");

    // 1. 定义地址：监听本地任意适配器，类型为经典蓝牙 (BR/EDR)
    let ctrl_addr = SocketAddr::new(Address::any(), AddressType::BrEdr, PSM_HID_CONTROL);
    let intr_addr = SocketAddr::new(Address::any(), AddressType::BrEdr, PSM_HID_INTERRUPT);

    // 2. 绑定监听器
    // 注意：在 Linux 上监听 PSM 17 和 19 属于低端口，通常需要 sudo 权限或 CAP_NET_BIND_SERVICE
    let ctrl_listener = StreamListener::bind(ctrl_addr)
        .await
        .map_err(|e| anyhow!("绑定控制通道失败 (PSM 17): {}. 是否缺少 root 权限？", e))?;

    let intr_listener = StreamListener::bind(intr_addr)
        .await
        .map_err(|e| anyhow!("绑定中断通道失败 (PSM 19): {}", e))?;

    println!("正在监听 L2CAP PSM 17(Control) 和 19(Interrupt)...");

    // 2. 使用 tokio::join! 同时等待两个连接
    // join! 会并发运行多个 future，直到它们全部完成
    let (ctrl_res, intr_res) = tokio::try_join!(
        async {
            let res = ctrl_listener.accept().await?;
            println!("控制通道(PSM 17)已连接: {:?}", res.1);
            Ok::<_, anyhow::Error>(res)
        },
        async {
            let res = intr_listener.accept().await?;
            println!("中断通道(PSM 19)已连接: {:?}", res.1);
            Ok::<_, anyhow::Error>(res)
        }
    )?;

    // 3. 存入 Socket（写入共享的 Arc<Mutex<...>>）
    *keyboard.control_socket.lock().await = Some(ctrl_res.0);
    *keyboard.interrupt_socket.lock().await = Some(intr_res.0);

    keyboard.adapter.set_discoverable(false).await?;
    keyboard.adapter.set_pairable(false).await?;

    println!("iPad 双通道已并发连接成功！");
    Ok(())
}

#[async_trait]
impl HidReportSender for BluetoothKeyboardHidDevice {
    /// 发送键盘报告
    async fn send_report(&mut self, report: InputReport) -> Result<()> {
        use tokio::io::AsyncWriteExt;

        if let InputReport::Keyboard { modifiers, keys } = report {
            let mut socket_guard = self.interrupt_socket.lock().await;
            if let Some(ref mut sock) = *socket_guard {
                // HID键盘报告格式: [Header, ReportID, Modifiers, Reserved, Key1-Key6]
                let mut hid_report = [
                    0xA1u8, 0x01, modifiers, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                ];

                // 填充按键数组 (最多6个按键)
                for (i, &key) in keys.iter().take(6).enumerate() {
                    hid_report[4 + i] = key;
                }

                sock.write_all(&hid_report).await?;
                sock.flush().await?;

                self.current_modifiers = KeyboardModifiers::from_bits_truncate(modifiers);
                self.current_keys.copy_from_slice(&hid_report[4..10]);
            }
        }

        Ok(())
    }
}

#[async_trait]
impl HidReportSender for BluetoothMouseHidDevice {
    /// 发送鼠标报告
    async fn send_report(&mut self, report: InputReport) -> Result<()> {
        use tokio::io::AsyncWriteExt;

        if let InputReport::Mouse {
            buttons,
            x,
            y,
            wheel,
        } = report
        {
            let mut socket_guard = self.interrupt_socket.lock().await;
            if let Some(ref mut sock) = *socket_guard {
                let x8 = x.clamp(-127, 127) as i8;
                let y8 = y.clamp(-127, 127) as i8;

                let hid_report = [0xA1, 0x02, buttons, x8 as u8, y8 as u8];

                sock.write_all(&hid_report).await?;
            }
        }

        Ok(())
    }
}

#[async_trait]
impl HidLedReader for BluetoothKeyboardHidDevice {
    /// 读取 LED 状态（如大写锁定等）
    async fn get_led_state(&mut self) -> Result<Option<LedState>> {
        // 返回默认状态
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::keycodes;
    use std::time::Duration;

    #[tokio::test]
    #[ignore]
    async fn test_bluetooth_connection() -> Result<()> {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

        let (keyboard, _mouse, session) = build_bluetooth_hid_device().await?;

        // 使用 Arc 包装以便在多个任务间共享
        let keyboard = Arc::new(Mutex::new(keyboard));
        let keyboard_clone = Arc::clone(&keyboard);

        // 启动服务器
        tokio::spawn(async move {
            let kbd = keyboard_clone.lock().await;
            if let Err(e) = run_server(&kbd, &session).await {
                eprintln!("服务器运行出错: {}", e);
            }
        });

        println!("--------------------------------------------------");
        println!("测试模式已启动！");
        println!("请在 iPad 的蓝牙设置中搜索并点击 'Virtual Keyboard Mouse'");
        println!("你有 60 秒时间完成配对和测试...");
        println!("--------------------------------------------------");

        // 等待连接
        for i in 0..600 {
            tokio::time::sleep(Duration::from_secs(1)).await;

            let mut kbd = keyboard.lock().await;
            let is_connected = kbd.interrupt_socket.lock().await.is_some();

            if is_connected {
                println!("检测到连接！发送一次按键 'A'...");

                // 按下 'A' 键
                let press_report = InputReport::Keyboard {
                    modifiers: 0x00,
                    keys: vec![keycodes::KEY_A],
                };
                kbd.send_report(press_report).await?;

                // 停顿一下
                tokio::time::sleep(Duration::from_millis(50)).await;

                // 松开所有按键
                let release_report = InputReport::Keyboard {
                    modifiers: 0x00,
                    keys: vec![],
                };
                kbd.send_report(release_report).await?;

                println!("'A' 键按下并松开完成。");
                break;
            } else if i % 10 == 0 {
                println!("等待中... ({}s)", i);
            }
        }

        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn test_mouse_drawing() -> Result<()> {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

        let (keyboard, mouse, session) = build_bluetooth_hid_device().await?;

        // 使用 Arc 包装
        let keyboard = Arc::new(Mutex::new(keyboard));
        let mouse = Arc::new(Mutex::new(mouse));
        let keyboard_clone = Arc::clone(&keyboard);

        let kbd = keyboard_clone.lock().await;
        if let Err(e) = run_server(&kbd, &session).await {
            eprintln!("服务器运行出错: {}", e);
        }

        println!("等待 iPad 连接以开始画图测试...");

        for i in 0..600 {
            tokio::time::sleep(Duration::from_secs(1)).await;

            let mut mouse_guard = mouse.lock().await;
            let is_connected = mouse_guard.interrupt_socket.lock().await.is_some();

            if is_connected {
                println!("连接成功！准备在屏幕上画一个正方形...");

                // 按下左键
                let press_report = InputReport::Mouse {
                    buttons: 0x01,
                    x: 0,
                    y: 0,
                    wheel: 0,
                };
                mouse_guard.send_report(press_report).await?;
                tokio::time::sleep(Duration::from_millis(100)).await;

                // 定义位移序列：右、下、左、上
                let movements = [
                    (10i16, 0i16), // 右
                    (0, 10),       // 下
                    (-10, 0),      // 左
                    (0, -10),      // 上
                ];

                for (dx, dy) in movements {
                    for _ in 0..20 {
                        let move_report = InputReport::Mouse {
                            buttons: 0x01, // 保持左键按下
                            x: dx,
                            y: dy,
                            wheel: 0,
                        };
                        mouse_guard.send_report(move_report).await?;
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                }

                // 松开左键
                let release_report = InputReport::Mouse {
                    buttons: 0x00,
                    x: 0,
                    y: 0,
                    wheel: 0,
                };
                mouse_guard.send_report(release_report).await?;

                println!("画图完成！");
                break;
            } else if i % 10 == 0 {
                println!("等待连接中... ({}s)", i);
            }
        }

        Ok(())
    }
}
