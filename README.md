<p align="center"><img src="assets/ghosty.png" width="140" alt="Ghosty" /></p>

# Ghosty Launch

> Que lo haga Ghosty. Clona un repo y lo publica **en vivo en producción** sobre las VMs de [EasyBits](https://www.easybits.cloud) — con tu cuenta y casi nada más.

Reemplaza el clásico `README` de 20 pasos por un TUI: te conectas a EasyBits, personalizas, y tu app queda corriendo en una microVM con URL pública. Filosofía **dev-in-prod**: no localhost, no Vercel, no Docker.

Es un **patrón reusable** — cualquier repo (CRM, ERP, lovable-clone, lo que sea) puede adoptarlo.

## Instalación

Un comando — instala **y arranca**:

```bash
sh -c "$(curl -fsSL https://raw.githubusercontent.com/blissito/ghosty-launch/main/install.sh)"
```

(En siguientes ocasiones, solo `ghosty-launch`.)

> Windows: baja el `.zip` desde [Releases](https://github.com/blissito/ghosty-launch/releases).
> macOS: el binario no está notarizado aún; el instalador le quita la cuarentena automáticamente.

## Cómo se usa

1. **Conéctate** — `enter` abre tu navegador para autorizar con EasyBits (OAuth, sin pegar llaves). La sesión se guarda; el próximo run reconecta solo. (`x` cierra sesión / cambia de cuenta.)
2. **Repo** — pega la URL de tu repo de GitHub (público).
3. **Personaliza** (solo apps) — nombre, color de acento y logo.
4. **Publica** — según el tipo de repo (ver abajo).
5. **En vivo** — tu URL pública. `o` abre, `b` vuelve al panel, `d` destruye.

**Lo único que necesitas:** una cuenta EasyBits (<a href="https://www.easybits.cloud/login" target="_blank" rel="noopener noreferrer">signup</a>).

## Usa Ghosty Launch en tu repo

Ghosty Launch detecta qué publica y elige el destino:

| Tipo | Detección | Destino | Costo |
|---|---|---|---|
| **Estático** (HTML/CSS/JS) | sin `package.json`, o `ghosty.toml type="static"` | **CDN** `easybits.cloud/s/<slug>` | sin cargo |
| **App con server** (Node) | hay `package.json` | **VM persistente** | cargo mensual |

Requisitos:
- Repo **público** (el clone no usa token aún).
- Apps: deben escuchar en **`0.0.0.0:$PORT`** (Ghosty inyecta `PORT=3000`), no en `localhost`.

### Contrato `ghosty.toml` (opcional, en la raíz del repo)

Solo lo necesitas si quieres forzar el tipo o dar una receta de build/arranque. Si falta, se auto-detecta.

```toml
# Forzar estático:
type = "static"

# O una app con receta de deploy (todo opcional; lo que falte se auto-detecta):
type = "app"
[deploy]
install = "npm ci"
build   = "npm run build"        # se corre solo si existe el script
start   = "npm start"

# Tamaño de la VM (opcional; si falta se detecta por el peso del repo):
[resources]
size = "l"                       # s | m | l | xl
```

Ejemplo, sitio estático sin build → **no necesitas archivo** (auto-detect). Si tu HTML necesita servirse con algo raro, declara `[deploy].start`.

### Tamaño de la VM (`[resources] size`)

Apps reales (RRv7, vite, Next) necesitan más RAM y disco que una micro-VM. Ghosty **detecta el peso del repo** y pide la clase adecuada; puedes forzarla con `[resources] size`.

| size | vCPU | RAM | disco | para |
|---|---|---|---|---|
| `s` | 1 | 512 MB | — | apps chicas sin build |
| `m` | 2 | 2 GB | 4 GB | build mediano |
| `l` | 4 | 4 GB | 12 GB | vite / RRv7 (apps pesadas) |
| `xl` | 8 | 8 GB | 24 GB | monorepos / Next.js |

`m`/`l`/`xl` requieren plan de pago en EasyBits (las VMs grandes cuestan). El estático (CDN) sigue **sin cargo**.

Apps personalizadas leen `APP_NAME` / `APP_ACCENT` / `APP_LOGO` del entorno (Ghosty los inyecta). Ver `examples/node-hello/`.

## Configuración (opcional)

| Var | Default | Qué hace |
|---|---|---|
| `EASYBITS_BASE_URL` | `https://www.easybits.cloud` | Apuntar a un EasyBits local/dev |
| `GHOSTY_REF_REPO` | `blissito/ghosty-ref-node` | Repo **prellenado** en la pantalla "repo" (lo puedes cambiar ahí) |

La app de referencia (server Node, lee `APP_NAME`/`APP_ACCENT`/`APP_LOGO`) está en `examples/node-hello/`.

Modo debug headless (sin TUI, para CI): `EASYBITS_API_KEY=eb_sk_… ghosty-launch --debug`.

## Stack

Rust · [ratatui](https://ratatui.rs) · tokio · reqwest · OAuth 2.1 (PKCE). Binarios para Linux/macOS/Windows.

## Licencia

Apache 2.0.
