pub fn init() {
    // 默认 info，可用 RUST_LOG 覆盖（例如 debug/trace）
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));

    // 统一日志格式：时间 + level + module + msg
    builder.format_timestamp_millis();
    builder.format_module_path(true);

    // 多次 init 不 panic（测试/多 task 场景更稳）
    let _ = builder.try_init();
}
