use forge::plugins::StandardPlugins;
use forge::{App, AppConfig, ForgeError, WindowHandle};
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

async fn greet(
    cfg: AppConfig,
    window: WindowHandle,
    req: GreetRequest,
) -> Result<GreetReply, ForgeError> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ForgeError::bad_request(
            "NAME_EMPTY",
            "Enter a name before greeting.",
        ));
    }

    let reply = GreetReply {
        message: format!("Hello, {name}! Powered by FORGE + Vue 3."),
        runtime: cfg.runtime.default.as_str().to_string(),
    };
    window.emit("greet.sent", &reply)?;
    Ok(reply)
}

fn main() -> forge::Result<()> {
    App::new()
        .plugin(StandardPlugins)
        .command("greet", greet)
        .run()
}
