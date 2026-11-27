fn main() -> sofie::core::Result<()> {
    sofie::run(saas::MODULES, saas::app, saas::worker)
}
