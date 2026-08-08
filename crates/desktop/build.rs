fn main() {
    // frontendDist (ui/dist) is gitignored; make sure it exists so fresh
    // clones can `cargo check` before ever running a frontend build.
    std::fs::create_dir_all("../../ui/dist").ok();
    tauri_build::build()
}
