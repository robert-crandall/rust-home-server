import { render } from "@solidjs/web";
import { App } from "./App";
import "./app.css";

const root = document.getElementById("app");
if (!root) {
  throw new Error("Missing #app mount element");
}

render(() => <App />, root);
