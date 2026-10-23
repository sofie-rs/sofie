fn main() -> sofie::core::Result<()> {
    sofie::run(
        clients_demo::MODULES,
        clients_demo::app,
        clients_demo::worker,
    )
}
