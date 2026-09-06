fn main() -> Result<(), Box<dyn std::error::Error>> {
    sex::cli::run(sex::cli::Frontend::Rate, std::env::args())
}
