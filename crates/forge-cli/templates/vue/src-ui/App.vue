<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";

interface GreetReply {
  message: string;
  runtime: string;
}

const name = ref("");
const reply = ref("");
const error = ref("");
const lastEvent = ref("");
const runtime = window.forge.runtime;

let unlisten: (() => void) | null = null;

onMounted(() => {
  unlisten = window.forge.listen<GreetReply>("greet.sent", (payload) => {
    lastEvent.value = `Rust event received on ${payload.runtime}`;
  });
});

onUnmounted(() => {
  unlisten?.();
});

async function onSubmit() {
  error.value = "";
  try {
    const res = await window.forge.invoke<GreetReply>("greet", { name: name.value });
    reply.value = `${res.message} (${res.runtime})`;
  } catch (err) {
    error.value = err instanceof Error ? err.message : String(err);
  }
}
</script>

<template>
  <main class="shell">
    <header class="header">
      <h1>{{APP_TITLE}}</h1>
      <span class="runtime-pill">runtime: {{ runtime }}</span>
    </header>

    <p class="lead">
      Edit <code>src-ui/App.vue</code> for instant Vue HMR or
      <code>src/main.rs</code> to add Rust commands.
    </p>

    <form class="card" novalidate @submit.prevent="onSubmit">
      <label for="name-input">Your name</label>
      <div class="row">
        <input
          id="name-input"
          v-model="name"
          type="text"
          placeholder="Ada Lovelace"
          autocomplete="off"
        />
        <button type="submit">Greet from Rust</button>
      </div>
      <p v-if="reply && !error" class="reply">{{ reply }}</p>
      <p v-if="error" class="reply is-error">{{ error }}</p>
      <p v-if="lastEvent" class="event-note">{{ lastEvent }}</p>
    </form>
  </main>
</template>
