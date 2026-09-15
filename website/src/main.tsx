import { render } from "svenjs";
import { App } from "./app";
import "./styles.css";

const root = document.getElementById("app");
if (!root) throw new Error("Missing #app");
render(<App />, root);
