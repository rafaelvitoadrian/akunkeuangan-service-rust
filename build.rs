fn main() {
    // Ensure newly added SQL migrations are embedded on the next build, too.
    println!("cargo:rerun-if-changed=migrations");
}
