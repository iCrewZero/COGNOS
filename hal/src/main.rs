//! HAL daemon entrypoint — starts the Human Approval Layer service.


fn main() {
    #[cfg(unix)]
    {
        env_logger::init();
        cognos_hal::HalDaemon::new().run();
    }

    #[cfg(not(unix))]
    {
        eprintln!("cognos-hal daemon requires a Unix platform.");
        std::process::exit(1);
    }
}
