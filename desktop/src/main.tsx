import { createRoot } from "react-dom/client";
import { App } from "./App";
import "../../web/src/styles/app.css";
import "./shell.css";
import "@kikimimi/ui/styles.css";
createRoot(document.getElementById("root")!).render(<App />);
