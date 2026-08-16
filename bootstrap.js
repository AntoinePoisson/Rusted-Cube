// The wasm module is pulled in dynamically on purpose. A static
// `import init from "./pkg/rusted_cube.js"` is resolved before any code in this
// file runs, so when that file is missing the whole module fails to load, the
// catch below never happens and the page sits on "Generating terrain" forever
// with nothing in the UI to say why.
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
  // by far the most common one: the page is served without the wasm-pack output
  // next to it, which is what happens when pkg/ never made it to the host
  if (/dynamically imported module|Failed to fetch|NetworkError|404/i.test(message)) {
    return "pkg/ is missing, build it with `wasm-pack build --target web --release`";
  }
  return message;
}

boot();
