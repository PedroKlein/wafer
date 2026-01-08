mod component;
mod normal;

use crate::component::component_host;
use crate::normal::normal_host;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    component_host()?;
    normal_host()?;
    Ok(())
}