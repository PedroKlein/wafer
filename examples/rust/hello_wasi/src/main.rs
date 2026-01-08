fn main() -> anyhow::Result<()> {
    println!("args = {:?}", std::env::args().collect::<Vec<_>>());
    println!("FOO = {:?}", std::env::var("FOO").ok());

    // Requires the host to grant directory access (preopen) for sandboxed FS.
    std::fs::read("Cargo.toml")?.iter().for_each(|b| print!("{}", *b as char));

    Ok(())
}



fn add(a: String, b: String) -> String {
    format!("{}{}", a, b)
}