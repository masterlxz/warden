import ReactDOM from "react-dom/client";
import App from "./App";

// No <StrictMode>: its double-mounted effects would open (and immediately close) a second hub
// connection on every load in development.
ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(<App />);
