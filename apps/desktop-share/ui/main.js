// Локальный пикер хабов. После connect_hub webview уходит на веб-интерфейс
// хаба — дальше работает существующий веб-фронт + мост __MATEHUB_NATIVE__.
const { invoke } = window.__TAURI__.core;

const el = (id) => document.getElementById(id);

function showError(message) {
  const p = el("connect-error");
  p.textContent = message;
  p.classList.toggle("hidden", !message);
}

async function connect(url) {
  showError("");
  el("connect").disabled = true;
  try {
    await invoke("connect_hub", { url });
    // Успех = навигация; эта страница выгружается.
  } catch (err) {
    showError(String(err));
    el("connect").disabled = false;
  }
}

async function refreshHubs() {
  try {
    const hubs = await invoke("list_hubs");
    el("recent").classList.toggle("hidden", hubs.length === 0);
    const list = el("hub-list");
    list.innerHTML = "";
    for (const hub of hubs) {
      const li = document.createElement("li");
      const btn = document.createElement("button");
      btn.type = "button";
      btn.textContent = hub;
      btn.addEventListener("click", () => connect(hub));
      li.appendChild(btn);
      list.appendChild(li);
    }
  } catch (err) {
    console.error("list_hubs failed", err);
  }
}

async function checkPermission() {
  try {
    const support = await invoke("capture_support");
    el("permission-banner").classList.toggle(
      "hidden",
      !support.supported || !!support.permission,
    );
  } catch (err) {
    console.error("capture_support failed", err);
  }
}

el("connect-form").addEventListener("submit", (e) => {
  e.preventDefault();
  let url = el("hub-url").value.trim();
  if (!url) return;
  if (!/^https?:\/\//i.test(url)) url = `https://${url}`;
  void connect(url);
});

el("request-permission").addEventListener("click", async () => {
  await invoke("request_capture_permission");
  await checkPermission();
});

void refreshHubs();
void checkPermission();
