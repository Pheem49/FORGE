<script lang="ts">
  interface GreetReply {
    message: string;
    runtime: string;
  }

  let name = $state("");
  let reply = $state("");
  let error = $state("");
  let lastEvent = $state("");
  const runtime = window.forge.runtime;

  $effect(() => {
    return window.forge.listen<GreetReply>("greet.sent", (payload) => {
      lastEvent = `Rust event received on ${payload.runtime}`;
    });
  });

  async function onSubmit(e: SubmitEvent) {
    e.preventDefault();
    error = "";
    try {
      const res = await window.forge.invoke<GreetReply>("greet", { name });
      reply = `${res.message} (${res.runtime})`;
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    }
  }
</script>

<main class="shell">
  <header class="header">
    <h1>{{APP_TITLE}}</h1>
    <span class="runtime-pill">runtime: {runtime}</span>
  </header>

  <p class="lead">
    Edit <code>src-ui/App.svelte</code> for instant Svelte 5 HMR or
    <code>src/main.rs</code> to add Rust commands.
  </p>

  <form class="card" novalidate onsubmit={onSubmit}>
    <label for="name-input">Your name</label>
    <div class="row">
      <input
        id="name-input"
        type="text"
        bind:value={name}
        placeholder="Ada Lovelace"
        autocomplete="off"
      />
      <button type="submit">Greet from Rust</button>
    </div>
    {#if reply && !error}
      <p class="reply">{reply}</p>
    {/if}
    {#if error}
      <p class="reply is-error">{error}</p>
    {/if}
    {#if lastEvent}
      <p class="event-note">{lastEvent}</p>
    {/if}
  </form>
</main>
