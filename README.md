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

1. **Conéctate** — `enter` abre tu navegador para autorizar con EasyBits (OAuth, sin pegar llaves). La sesión se guarda; el próximo run reconecta solo.
2. **Personaliza** — nombre, color de acento (presets o hex custom) y logo (ruta/URL, o arrastra el archivo).
3. **Publica** — Ghosty levanta tu VM, clona el repo en `/app`, instala, arranca y expone el puerto.
4. **En vivo** — tu URL `https://sb-…easybits.cloud`. `o` abre, `r` re-publica, `d` destruye.

**Lo único que necesitas:** una cuenta EasyBits ([signup](https://www.easybits.cloud)). 

⚠️ La VM publicada es **persistente (always-on) = cargo mensual** en tu cuenta EasyBits hasta que la destruyas (`d`).

## Configuración (opcional)

| Var | Default | Qué hace |
|---|---|---|
| `EASYBITS_BASE_URL` | `https://www.easybits.cloud` | Apuntar a un EasyBits local/dev |
| `GHOSTY_REF_REPO` | `blissito/ghosty-ref-node` | Repo a clonar y desplegar en la VM |

La app de referencia (server Node, lee `APP_NAME`/`APP_ACCENT`/`APP_LOGO`) está en `examples/node-hello/`.

Modo debug headless (sin TUI, para CI): `EASYBITS_API_KEY=eb_sk_… ghosty-launch --debug`.

## Stack

Rust · [ratatui](https://ratatui.rs) · tokio · reqwest · OAuth 2.1 (PKCE). Binarios para Linux/macOS/Windows.

## Licencia

Apache 2.0.
