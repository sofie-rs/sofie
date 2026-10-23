// `sofie dev` only: one held request at a time asks whether the server restarted or anything changed.
(() => {
  "use strict";
  let server = null;
  let generation = "";

  const fresh = (path, version) => `${path.split("?")[0]}?${version}`;

  function reloadIslands(version) {
    for (const element of document.querySelectorAll("[data-sofie-islands]")) {
      if (element.localName === "link") {
        element.href = fresh(element.href, version);
      } else {
        import(fresh(element.src, version));
      }
    }
  }

  function changed(answer) {
    const [boot, next, kind] = answer.trim().split(" ");
    if (server !== null && boot !== server) {
      location.reload();
      return false;
    }
    const isNew = server !== null && next !== generation;
    server = boot;
    generation = next;
    if (isNew && kind === "islands") {
      reloadIslands(next);
    } else if (isNew) {
      location.reload();
      return false;
    }
    return true;
  }

  async function ask() {
    try {
      const response = await fetch(`/_sofie/dev/reload?after=${generation}`, { cache: "no-store" });
      if (changed(await response.text())) {
        ask();
      }
    } catch {
      // The server is restarting: ask again until it answers.
      setTimeout(ask, 300);
    }
  }

  ask();
})();
