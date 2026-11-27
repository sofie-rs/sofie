fn main() -> sofie::core::Result<()> {
    sofie::run_with_flags(saas::MODULES, saas::app, saas::worker)
}
