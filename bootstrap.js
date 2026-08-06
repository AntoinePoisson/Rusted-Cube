import init from "./pkg/rusted_cube.js";

async function boot() {
  try {
    await init();
  } catch (error) {
    const loader = document.querySelector("#loader");
    if (loader) {
      loader.textContent = "Unable to start Rusted Cube";
    }
    console.error(error);
  }
}

boot();
