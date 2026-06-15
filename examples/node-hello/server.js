// App de referencia mínima para probar el pipeline de Ghosty Launch.
// Sin dependencias, sin build: solo escucha en $PORT y responde.
const http = require("http");

const port = process.env.PORT || 3000;
// Personalización inyectada por Ghosty Launch (paso "Personaliza").
const name = process.env.APP_NAME || "Ghosty Launch";
const accent = process.env.APP_ACCENT || "#a78bfa";
const logo = process.env.APP_LOGO || "";

const logoHtml = logo
  ? `<img src="${logo}" alt="${name}" style="max-width:160px;max-height:160px;margin-bottom:20px" />`
  : `<h1>👻 ${name}</h1>`;

http
  .createServer((_req, res) => {
    res.writeHead(200, { "Content-Type": "text/html; charset=utf-8" });
    res.end(
      `<!doctype html><html><head><meta charset="utf-8"><title>${name}</title></head>` +
        `<body style="font-family:system-ui;display:grid;place-items:center;height:100vh;margin:0;background:#0b0b0f;color:${accent}">` +
        `<div style="text-align:center">${logoHtml}` +
        (logo ? `<h2>${name}</h2>` : "") +
        `<p style="color:#787882">Publicado por Ghosty Launch — dev in prod.</p></div>` +
        `</body></html>`
    );
  })
  .listen(port, () => console.log(`${name} escuchando en :${port}`));
