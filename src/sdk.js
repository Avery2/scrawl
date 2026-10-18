// Scrawl dev SDK, served by the Scrawl app at http://127.0.0.1:47777/sdk.js.
// <script src="http://127.0.0.1:47777/sdk.js" data-session="ses_…"></script>
// or data-directory="/abs/project" to use that project's newest OpenCode session.
// Registers this tab with Scrawl while it's open, so a Mac capture of it is sent to that
// session. It does nothing outside localhost unless data-token (the ingest token) is set.
(() => {
  const script = document.currentScript;
  if (!script || window.__scrawlSdk) return;
  const host = location.hostname;
  const loopback =
    host === "localhost" ||
    host.endsWith(".localhost") ||
    host === "[::1]" ||
    host === "::1" ||
    /^127\./.test(host);
  const token = script.dataset.token;
  if (!loopback && !token) return;
  window.__scrawlSdk = true;

  const base = new URL(script.src).origin;
  const tab = Math.random().toString(36).slice(2, 14);
  // text/plain keeps loopback registrations simple requests (no preflight).
  const headers = token
    ? { "Content-Type": "application/json", Authorization: "Bearer " + token }
    : { "Content-Type": "text/plain" };
  const send = (path, keepalive) =>
    fetch(base + path, {
      method: "POST",
      mode: "cors",
      keepalive,
      headers,
      body: JSON.stringify({
        origin: location.origin,
        url: location.href,
        title: document.title,
        session: script.dataset.session || undefined,
        directory: script.dataset.directory || undefined,
        tab,
      }),
    }).catch(() => {});

  const beat = () => {
    if (document.visibilityState !== "hidden") send("/web/register");
  };
  beat();
  setInterval(beat, 30000);
  document.addEventListener("visibilitychange", beat);
  addEventListener("pageshow", beat);
  addEventListener("pagehide", () => send("/web/unregister", true));
})();
