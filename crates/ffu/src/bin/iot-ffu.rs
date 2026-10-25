fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let status = match wor_ffu::run(&args) {
        Ok(status) => status,
        Err(error) => {
            eprintln!("IoT Core FFU: {error}");
            error.exit_status()
        }
    };
    std::process::exit(status);
}
