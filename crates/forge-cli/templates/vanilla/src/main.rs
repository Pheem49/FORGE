use forge::{App, AppConfig, ForgeError};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct GreetRequest {
    name: String,
}

#[derive(Serialize)]
struct GreetReply {
    message: String,
    runtime: String,
}

async fn greet(cfg: AppConfig, req: GreetRequest) -> Result<GreetReply, ForgeError> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ForgeError::bad_request(
            "NAME_EMPTY",
            "Enter a name before greeting.",
        ));
    }

    Ok(GreetReply {
        message: format!("Hello, {name}! Built with FORGE."),
        runtime: cfg.runtime.default.as_str().to_string(),
    })
}

fn main() -> forge::Result<()> {
    App::new().command("greet", greet).run()
}
