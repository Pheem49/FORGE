import { FormEvent, useEffect, useState } from "react";

interface GreetReply {
  message: string;
  runtime: string;
}

export function App() {
  const [name, setName] = useState("");
  const [reply, setReply] = useState<string>("");
  const [error, setError] = useState<string>("");
  const [lastEvent, setLastEvent] = useState<string>("");

  useEffect(() => {
    return window.forge.listen<GreetReply>("greet.sent", (payload) => {
      setLastEvent(`Rust event received on ${payload.runtime}`);
    });
  }, []);

  async function onSubmit(e: FormEvent) {
    e.preventDefault();
    setError("");
    try {
      const res = await window.forge.invoke<GreetReply>("greet", { name });
      setReply(`${res.message} (${res.runtime})`);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <main className="shell">
      <header className="header">
        <h1>{{APP_TITLE}}</h1>
        <span className="runtime-pill">runtime: {window.forge.runtime}</span>
      </header>

      <p className="lead">
        Edit <code>src-ui/App.tsx</code> for instant React HMR or{" "}
        <code>src/main.rs</code> to add Rust commands.
      </p>

      <form onSubmit={onSubmit} className="card" noValidate>
        <label htmlFor="name-input">Your name</label>
        <div className="row">
          <input
            id="name-input"
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Ada Lovelace"
            autoComplete="off"
          />
          <button type="submit">Greet from Rust</button>
        </div>
        {reply && !error && <p className="reply">{reply}</p>}
        {error && <p className="reply is-error">{error}</p>}
        {lastEvent && <p className="event-note">{lastEvent}</p>}
      </form>
    </main>
  );
}
