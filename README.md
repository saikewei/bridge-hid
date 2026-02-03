# Bridge HID

[English](#english) | [中文](#chinese)

<a name="english"></a>

## English

Bridge HID is a high-performance HID (Keyboard/Mouse) switcher implemented in Rust. It allows you to use one set of physical keyboard and mouse to control two different hosts: one via **USB Gadget (Wired)** and another via **Bluetooth BLE**.

### Features
- **Dual Output**: Seamlessly switch between USB wired connection and Bluetooth BLE connection.
- **Low Latency**: Optimized event processing for gaming and professional use.
- **Auto-Sync**: Synchronizes Keyboard LED states (NumLock, CapsLock) across devices.
- **Raspberry Pi Optimized**: Designed to run on Raspberry Pi Zero / 4 / 5 using the USB gadget mode.

### Prerequisites (Raspberry Pi Configuration)
To use USB Gadget mode, you must enable the `dwc2` driver on your Raspberry Pi:

1. **Enable DWC2 Overlay**:
   Edit `/boot/config.txt` (or `/boot/firmware/config.txt` on newer OS):
   ```bash
   echo "dtoverlay=dwc2" | sudo tee -a /boot/config.txt
   ```

2. **Enable DWC2 Module**:
   Edit `/etc/modules`:
   ```bash
   echo "dwc2" | sudo tee -a /etc/modules
   ```

3. **Reboot**:
   ```bash
   sudo reboot
   ```

### How to Run
Since the program interacts directly with input devices (`/dev/input/`) and USB gadget files, it requires root privileges.

```bash
cargo build --release
sudo ./target/release/bridge-hid
```

### Switching Output
The default shortcut to toggle between USB and Bluetooth is:
**`Ctrl + Alt + F12`**

---

<a name="chinese"></a>

## 中文

Bridge HID 是一个基于 Rust 开发的高性能 HID（键鼠）切换器。它允许您使用一套物理键盘和鼠标同时控制两台主机：一台通过 **USB Gadget (有线)** 连接，另一台通过 **蓝牙 BLE** 连接。

### 功能特性
- **双模输出**: 在 USB 有线连接与蓝牙 BLE 连接之间无缝切换。
- **低延迟**: 针对输入操作进行了优化，适用于办公及游戏场景。
- **状态同步**: 自动同步不同主机间的键盘 LED 状态（如大写锁定、数字键盘锁）。
- **树莓派优化**: 专门针对支持 USB Gadget 模式的树莓派（如 Zero, 4, 5）设计。

### 前置条件 (树莓派配置)
在使用 USB Gadget 模式前，必须开启树莓派的 `dwc2` 驱动：

1. **开启 DWC2 叠加层**:
   编辑 `/boot/config.txt` (新版系统为 `/boot/firmware/config.txt`):
   ```bash
   echo "dtoverlay=dwc2" | sudo tee -a /boot/config.txt
   ```

2. **启用 DWC2 模块**:
   编辑 `/etc/modules`:
   ```bash
   echo "dwc2" | sudo tee -a /etc/modules
   ```

3. **重启设备**:
   ```bash
   sudo reboot
   ```

### 如何运行
由于程序需要直接访问输入设备 (`/dev/input/`) 和 USB gadget 系统文件，因此需要 `sudo` 权限运行。

```bash
cargo build --release
sudo ./target/release/bridge-hid
```

### 切换输出
默认的 USB/蓝牙 切换快捷键为：
**`Ctrl + Alt + F12`**

