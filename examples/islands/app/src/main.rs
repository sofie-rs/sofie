fn main() -> sofie::core::Result<()> {
    sofie::run(
        islands_demo::MODULES,
        islands_demo::app,
        islands_demo::worker,
    )
}
