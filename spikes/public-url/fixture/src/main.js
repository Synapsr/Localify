import { depth, total } from "./generated/index.js";
import { message } from "./hmr-target.js";

const target = document.querySelector("#hmr");
target.textContent = message;

if (import.meta.hot) {
  import.meta.hot.accept("./hmr-target.js", (next) => {
    target.textContent = next.message;
  });
}

window.__spike = { total, depth, loadedAt: performance.now() };
