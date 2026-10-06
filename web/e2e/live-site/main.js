// A dev-server app for the Chrome overlay's browser test: a form whose
// button label the test edits, so Vite's hot update re-renders #app.
const app = document.querySelector("#app");
const LABEL = "Save";
app.innerHTML = `<main><h1>Settings</h1><form><input type="password" name="pw" value="hunter2"><input type="hidden" name="csrf" value="tok123">${LABEL === "" ? "" : `<button id="save" type="button">${LABEL}</button>`}</form></main>`;
if (import.meta.hot) import.meta.hot.accept();
