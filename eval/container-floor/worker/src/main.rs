use std::io;

fn main() -> io::Result<()> {
    io::copy(&mut io::stdin().lock(), &mut io::stdout().lock())?;
    Ok(())
}
