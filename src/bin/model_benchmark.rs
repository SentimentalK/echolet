fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    echolet::diagnostics::benchmark::run_cli(&args)
}
