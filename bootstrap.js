// A dynamic import lets us report a useful error when the wasm-pack output is missing.
async function boot() {
  try {
    const { default: init } = await import("./pkg/rusted_cube.js");
    await init();
  } catch (error) {
    console.error(error);
    reportFailure(error);
  }
}

function reportFailure(error) {
  const loader = document.querySelector("#loader");
  if (!loader) {
    return;
  }
  loader.textContent = `Unable to start Rusted Cube - ${describe(error)}`;
  loader.classList.add("loader--failed");
}

function describe(error) {
  const message = String(error?.message ?? error);
  if (/dynamically imported module|Failed to fetch|NetworkError|404/i.test(message)) {
    return "pkg/ is missing, build it with `wasm-pack build --target web --release`";
  }
  return message;
}

boot();
