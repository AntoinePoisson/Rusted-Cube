const root = document.documentElement;
const coarsePointer = window.matchMedia?.("(pointer: coarse)").matches;
const requestedInput = new URLSearchParams(window.location.search).get("input");
root.dataset.input =
  requestedInput === "touch" || coarsePointer || navigator.maxTouchPoints > 0
    ? "touch"
    : "pointer";

const canvas = document.querySelector("#game-canvas");
canvas?.addEventListener(
  "webglcontextlost",
  (event) => {
    event.preventDefault();
    reportFailure(new Error("The graphics context was lost. Reload the page to continue."), true);
  },
  { passive: false },
);

canvas?.addEventListener("webglcontextrestored", () => {
  reportFailure(new Error("Graphics are available again. Reload the page to rebuild the world."), true);
});

// A dynamic import lets us report a useful error when the wasm-pack output is missing.
async function boot() {
  try {
    const { default: init } = await import("./pkg/rusted_cube.js");
    await init();
  } catch (error) {
    console.error(error);
    reportFailure(error, false);
  }
}

function reportFailure(error, canReload) {
  const loader = document.querySelector("#loader");
  if (!loader) {
    return;
  }
  loader.replaceChildren();
  loader.className = "loader loader--failed";

  const message = document.createElement("span");
  message.textContent = describe(error);
  loader.append(message);

  if (canReload) {
    const reload = document.createElement("button");
    reload.type = "button";
    reload.textContent = "Reload";
    reload.addEventListener("click", () => window.location.reload());
    loader.append(reload);
  }
}

function describe(error) {
  const message = String(error?.message ?? error);
  if (/dynamically imported module|Failed to fetch|NetworkError|404/i.test(message)) {
    return "The game files could not be loaded. Check your connection and try again.";
  }
  return message;
}

boot();
