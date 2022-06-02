use poison_guard::Poison;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut state = Poison::new(42);

    let mut guard = Poison::unless_recovered(&mut state)?;

    Poison::try_recover(use_state(&mut guard).await, guard)?;

    Ok(())
}

async fn use_state(state: &mut i32) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tokio::time::sleep(Duration::from_secs(1)).await;

    if *state > 3 {
        Err("too much state!".into())
    } else {
        Ok(())
    }
}
