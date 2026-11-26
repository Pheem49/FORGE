(function () {
  "use strict";

  const pill = document.getElementById("runtime-pill");
  const form = document.getElementById("greet-form");
  const input = document.getElementById("name-input");
  const reply = document.getElementById("reply");

  pill.textContent = `runtime: ${window.forge.runtime}`;

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    reply.classList.remove("is-error");
    try {
      const res = await window.forge.invoke("greet", { name: input.value });
      reply.textContent = `${res.message} (${res.runtime})`;
    } catch (err) {
      reply.textContent = err.message;
      reply.classList.add("is-error");
    }
  });
})();
