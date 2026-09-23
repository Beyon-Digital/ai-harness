const params = new URLSearchParams(location.search);
const msg = params.get("msg");
if (msg) document.getElementById("msg").textContent = msg;
const dir = params.get("dir");
if (dir) {
  document.getElementById("datadir").textContent = `App data: ${dir}`;
}

// Injected by Tauri (withGlobalTauri) — hide the actions when the page
// is opened outside the app shell (e.g. plain browser debugging).
const invoke = window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke;
const retry = document.getElementById("retry");
const openLogs = document.getElementById("open-logs");
if (!invoke) {
  retry.style.display = "none";
  openLogs.style.display = "none";
} else {
  openLogs.addEventListener("click", () => {
    invoke("open_logs").catch(() => {});
  });
  retry.addEventListener("click", () => {
    retry.disabled = true;
    retry.textContent = "Retrying…";
    invoke("retry_startup").catch(() => {
      retry.disabled = false;
      retry.textContent = "Retry";
    });
  });
}
