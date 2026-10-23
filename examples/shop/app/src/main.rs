fn main() -> sofie::core::Result<()> {
    sofie::run(shop::MODULES, shop::app, shop::worker)
}
