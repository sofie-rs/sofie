// A passkey ceremony in a client area, as `engine.js` runs it on an HTML page: a button with
// `data-sofie-passkey` ("create" or "get") holds the server's options in `data-options`; the
// browser's answer goes to `data-post` as JSON. A reply's `location` loads as a whole page, as
// signing in changes the area; otherwise the screen loads again.

const NOT_ACCEPTED = "The passkey was not accepted. Try again.";
const NOT_USED = "The passkey was not used. Try again.";
const UNSUPPORTED = "This browser cannot use passkeys.";

// As the server's `is_followed`: `https:` or this site, so a replied value cannot run a script.
export function isFollowed(target) {
  if (typeof target !== "string") return false;
  try {
    const url = new URL(target, location.href);
    return url.protocol === "https:" || url.origin === location.origin;
  } catch {
    return false;
  }
}

export function passkey(event, runtime) {
  const button = event.target.closest?.("[data-sofie-passkey]");
  if (!button) return;
  event.preventDefault();
  if (!window.PublicKeyCredential) {
    runtime.notify([UNSUPPORTED]);
    return;
  }
  ceremony(button, runtime).catch(() => runtime.notify([NOT_USED]));
}

async function ceremony(button, runtime) {
  const options = JSON.parse(button.dataset.options);
  options.challenge = bytes(options.challenge);
  let credential;
  if (button.dataset.sofiePasskey === "create") {
    options.user.id = bytes(options.user.id);
    options.excludeCredentials = credentials(options.excludeCredentials);
    credential = await navigator.credentials.create({ publicKey: options });
  } else {
    options.allowCredentials = credentials(options.allowCredentials);
    credential = await navigator.credentials.get({ publicKey: options });
  }
  const answer = await fetch(button.dataset.post, {
    method: "POST",
    credentials: "same-origin",
    headers: { "Content-Type": "application/json", Accept: "application/json" },
    body: JSON.stringify(answered(credential)),
  });
  const reply = await answer.json().catch(() => ({}));
  if (!answer.ok) {
    runtime.notify([reply.error || NOT_ACCEPTED]);
    return;
  }
  if (isFollowed(reply.location)) location.assign(reply.location);
  else await runtime.load(location.href, false);
}

function answered(credential) {
  const response = credential.response;
  const body = { id: credential.id, client_data: text(response.clientDataJSON) };
  if (response.attestationObject) body.attestation_object = text(response.attestationObject);
  if (response.authenticatorData) body.authenticator_data = text(response.authenticatorData);
  if (response.signature) body.signature = text(response.signature);
  return body;
}

function credentials(list) {
  return (list || []).map((credential) => ({ type: credential.type, id: bytes(credential.id) }));
}

function bytes(base64url) {
  const binary = atob(base64url.replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}

function text(buffer) {
  const binary = String.fromCharCode(...new Uint8Array(buffer));
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
