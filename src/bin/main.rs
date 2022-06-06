use anyhow::Error;

fn run() -> Result<(), Error> {
    println!("Hello, world!");

    Ok(())
}

fn main() -> Result<(), Error> {
    run()?;
    Ok(())
}