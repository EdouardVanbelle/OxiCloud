# OxiCloud — Roadmap UI/UX: de 4.8 a 10

> Plan de trabajo derivado de la auditoría multi-agente del frontend (`/static`).
> Cada tarea está anclada a archivos reales. Las fases son secuenciales: **Fase 0
> (fundamentos) debe ir primero** porque casi todo lo demás depende de los tokens.
> Esfuerzo: **S** <2h · **M** ~medio día · **L** ~1-2 días · **XL** ~3 días+.

## Trayectoria de puntuación

| Fase | Qué entrega | Nota al terminar |
|------|-------------|:----------------:|
| — | Estado actual | **4.8** |
| **0 · Fundamentos** | Las 6 escalas de tokens que faltan (espaciado, tipo, radios, elevación, z-index, movimiento) + rampas de color OKLCH + higiene | **5.3** |
| **1 · Migración** | Aplicar las escalas en los ~55 archivos CSS, gobernar la paleta, unificar la marca en superficies externas, arreglar jerarquía de headings | **6.5** |
| **2 · Estados y A11y** | `:focus-visible`, disabled/loading, contraste WCAG AA, navegación por teclado, landmarks, reduced-motion, ARIA, touch targets | **8.0** |
| **3 · Pulido** | Empty states + skeletons, placeholders de fotos, lightbox completo, feel del movimiento, login atmosférico, reflow responsive, voz de marca | **9.2** |
| **4 · Clase mundial + guardarraíles** | Command palette, View Transitions, undo, i18n con Intl, offline, onboarding, microcopy + **CI (axe, contraste, visual regression) y design system documentado que sostienen el 10** | **10** |

---

## FASE 0 — Fundamentos (las escalas de tokens que faltan)

> El problema raíz de la auditoría: solo el color está sistematizado. Aquí se crean
> los ejes que faltan en [variables.css](../static/css/base/variables.css). 4-5 áreas
> propusieron estas mismas tareas de forma independiente → son la base correcta.

### Escalas de tokens ✅ (resuelto — definidas en [variables.css](../static/css/base/variables.css), [reset.css](../static/css/base/reset.css), [typography.css](../static/css/base/typography.css))
- [x] **f0-space** (S) — Escala de espaciado `--space-*` en grid de 4px (`--space-0`…`--space-24`, con medios pasos 2/6/10/14px). _Migración de los px crudos → Fase 1._
- [x] **f0-radius** (S) — Escala `--radius-*` (`xs`…`4xl`,`full`) y **token huérfano `--radius` ya definido** (alias de `--radius-2xl`; resuelve los `var(--radius,12px)` de share-public/device-verify).
- [x] **f0-type** (M) — Escala tipográfica en rem `--text-2xs`…`--text-6xl` con `--leading-*`/`--weight-*`/`--tracking-*`, `--font-sans`/`--font-mono`, `--measure-prose` (65ch) y `--icon-*` separada del texto.
- [x] **f0-type-base** (S) — `font-size: var(--text-base)` + `line-height: 1.5` en `body`; `html { font-size: 100% }` (honra zoom); clases `.heading-page/-section/-card/-eyebrow` + `.prose` en typography.css.
- [x] **f0-zindex** (S) — Capas semánticas `--z-*` (base…max) con huecos para insertar.
- [x] **f0-motion** (S) — `--motion-*` (instant…spinner) + `--ease-*` con **5 curvas `cubic-bezier`** (standard/emphasized de desaceleración por defecto) + `--spin-duration`.
- [x] **f0-elevation** (M) — Recetas `--shadow-xs…2xl` compuestas; **`--color-shadow-5xl` muerto eliminado** y **colisión dark arreglada** (base 0.3 < md 0.34 < lg 0.38).
- [~] **f0-breakpoints** (S) — Tokens `--bp-xs…xl` definidos como referencia para JS/docs. ⚠️ **Diferido (razón real, no "dependencia"):** lightningcss SÍ soporta `@custom-media`, pero `web/mod.rs` sirve el CSS **crudo desde `static/`** en debug (`cargo run`), sin preprocesar. `@custom-media` no tiene soporte nativo de navegador → `@media (--bp-sm)` rompería **todas** las media queries en dev. Solo viable si dev también preprocesa CSS (no lo hace). Las custom props tampoco funcionan en condiciones `@media` de forma nativa. Quedan como referencia.
- [x] **f0-density** (S) — Tokens `--density-*` (row/gap/control) + override `html[data-density="compact"]`. _Consumidos por filas/controles en Fase 1._
- [x] **f0-sidebar-fluid** (S) — `--sidebar-width: clamp(220px,18vw,280px)` + tokens min/max/collapsed. _Rail redimensionable → Fase 1._
- [x] **f0-dvh** (S) — `body { height:100vh; height:100dvh }` con fallback. _Migración de `100vh` en views → Fase 1 (f1-vh-views)._

### Fundamentos de color (curado, Opción B) ✅ — validado WCAG por script
- [~] **f0-oklch-ramps** (L) — **Adaptado:** curé los semánticos directamente a un hue sobrio cada uno con valores validados; **no** construí la capa formal de primitivas OKLCH (arquitectura opcional a futuro — el resultado de armonización ya se logró).
- [~] **f0-offwhites** (M) — **Diferido:** bajo valor y riesgo de aplanar el micro-layering de superficies sin poder renderizar. Revisar con la app en navegador.
- [x] **f0-brand-orange** (M) — Naranja confirmado como único acento de marca; `--color-accent-text`/`--color-on-accent` añadidos. _Degradar azul/índigo/púrpura → Fase 1 (f1-demote-*)._
- [x] **f0-textgray** (M) — 14 grises → 4 tiers AA-safe (secondary/muted/subtle/faint) + **todos los nombres legacy como alias** (0 roturas). muted/faint/placeholder **oscurecidos para pasar AA** (antes 2.3–4.0:1). _Adelanta f2-contrast-text._
- [x] **f0-semantic-success** (L) — Unificado a esmeralda sobrio (`#16a34a`); 6 verdes → alias canónicos.
- [x] **f0-semantic-warning** (L) — Unificado a ámbar (`text #b45309` AA, `border #f59e0b`); el oro brillante `#ffc107` se retira. 7 variantes → alias.
- [x] **f0-semantic-danger** (L) — Unificado a `#ef4444/#dc2626`; `danger-text-alt` ahora usa el rojo AA.
- [x] **f0-semantic-info** (L) — Unificado a azul (`text #1d4ed8` AA + tier dark); variantes → alias. _Calendar dots + iconos de nav regenerados a familia armonizada (S55/L62)._

### Tokens de acento, foco y marca
- [x] **f0-accent-text** (S) — `--color-accent-text: light-dark(#d23c18, #ff8a5c)` + `--color-focus-ring` añadidos. _Migración de enlaces → Fase 2 (f2-contrast-link)._
- [x] **f0-on-accent** (S) — `--color-on-accent: #ffffff` añadido. _Repunteo de consumidores de `--color-danger-text` → Fase 1._
- [~] **f0-brand-mark** (M) — **Parcial:** `--color-logo-gradient` canónico definido (unifica los dos gradientes divergentes). _SVG único + componente de lockup + wordmark/settings-layout tokens → Fase 1 (f1-logo-*, f1-settings-container)._

### Higiene y andamiaje (rápidas, habilitan el resto) ✅
- [x] **f0-stylelint-typo** (S) — Glob arreglado (`statc`→`static`). **0 hex/rgba crudos** fuera de variables/themes → CI no se romperá por colores. _(Auditoría 2026-06: una fase posterior había metido 3 hex en `base/a11y.css` (`@media prefers-contrast`) que stylelint habría marcado; movidos a `variables.css` (capa de tokens, whitelisted) → invariante restaurado y reverificado.)_
- [x] **f0-debug-red** (S) — Los tokens "debug" sí se usaban (badge de conteo de drag); **repunteados** a `--color-notification-badge`/`--color-danger-text` y eliminados de variables.css.
- [x] **f0-legacy-badge** (S) — Bloque FIXME (11 tokens `*-dark-*`) **eliminado** — confirmado 0 consumidores.
- [x] **f0-animations-file** (S) — [base/animations.css](../static/css/base/animations.css) creado con el `@keyframes spin` canónico, conectado temprano en main.css.
- [x] **f0-delete-spin-dupes** (S) — 6 `@keyframes spin` duplicados eliminados. _`oxi-spin`/`smdSpin` (1 uso c/u) diferidos a Fase 1 (requieren tocar consumidores)._
- [x] **f0-dead-upload-toast** (S) — Bloque muerto `.upload-toast` (162 líneas) + `<div>` oculto de index.html **eliminados** (0 referencias JS).
- [x] **f0-sr-only** (S) — `.sr-only` añadida en reset.css.
- [~] **f0-perf-baseline** (M) — **Diferida:** requiere correr la app + build + Lighthouse (no disponible en este entorno). El archivo de presupuestos se crea junto al primer baseline.

---

## FASE 1 — Migración (aplicar las escalas en todo el código)

### Migrar CSS a las escalas ✅ — codemod value-preserving (1645 reemplazos, 41 archivos, 0 cambio visual)
- [x] **f1-mig-base** (S) — forms.css migrado (reset.css ya estaba; main.css no tiene valores).
- [x] **f1-mig-layout** (M) — sidebar/topbar/content migrados.
- [x] **f1-mig-comp-core** (L) — buttons/fileManager/resourceList/fileType/breadcrumb migrados.
- [x] **f1-mig-comp-overlays** (L) — dialogs/modals/shareModal/groupsModal/shareDialog/contextMenu/uploadDropdown migrados.
- [x] **f1-mig-comp-misc** (L) — notifications/tooltip/itemTooltip/search/spinner/batchToolbar/chips/userVignette/userMenu/languageSelector/csp-utilities migrados.
- [x] **f1-mig-views** (XL) — los 14 views migrados (admin/music/profile/auth/photos/trash/recent/favorites/sharedWithMe/mySharesView/inlineViewer/device-verify/share-public/photosLightbox).
- [x] **f1-weight-normalize** (M) — `bold`/`normal` + numéricos → tokens `--weight-*`.
- [x] **f1-leading-normalize** (S) — `line-height` unitless exactos → tokens `--leading-*`.
- [~] **f1-snap-fractional** (M) — **Parcial:** los exactos ya van por token; quedan **184 px off-grid** (15/17/11/13px) sin token exacto → requieren decisión de snapping (cambio visual). _Pendiente._
- [ ] **f1-mig-root-dups** (M) — Reconciliar shims raíz vs views/ (separado de la migración de valores).
- [~] **f1-mig-motion-sweep** (L) — **Diferido:** las duraciones dominantes (0.15s/0.25s) no tienen token exacto → tokenizarlas es un cambio de *feel*, no value-preserving. Decisión aparte.
- [ ] **f1-spin-duration-token** (S) — Diferido (los spinners usan 0.7s/1s; unificar a `--spin-duration` cambia el valor).
- [ ] **f1-icon-size-scale** (M) — Diferido (separar `font-size` de icono vs texto no es automatizable sin ambigüedad).
- [ ] **f1-tracking-normalize** (M) — Diferido (los `letter-spacing` en px/em no tienen token exacto).

### Jerarquía de headings ✅ — cada página con un único h1, sin saltos
- [x] **f1-h-index** (S) — modal-title `h3→h2` (selector `.modal-header h2,h3` actualizado) → orden `h1→h2→h2`.
- [x] **f1-h-login** (S) — `<h1 class="sr-only">OxiCloud</h1>` añadido (usa la utilidad `.sr-only` de Fase 0); paneles quedan h2.
- [x] **f1-h-admin** (M) — `<h1 class="sr-only">Admin panel</h1>` añadido; secciones h2, subsecciones h3.
- [x] **f1-h-profile** (S) — panel de error `h2→h1` (queda h1 propio en cada panel excluyente).
- [x] **f1-h-rest** (S) — share/device-verify ya tenían h1 (verificado); nextcloud-login/success/error `h2→h1`.

### Gobernanza de paleta ✅ (parcial donde es decorativo)
- [~] **f1-fileicon-pairs** (M) — **Diferido:** re-derivar pares bg/text de iconos (pdf/doc/image/audio/video) de los semánticos — es pulido visual, revisar con la app.
- [—] **f1-fileicon-langs** (L) — **Descartado** por tu elección (mantener colores por lenguaje).
- [x] **f1-calendar-dots** (M) — Hecho en Fase 0 (familia armonizada S55/L62).
- [x] **f1-demote-blue** (M) — `--color-primary` aliasado a `--color-accent` → naranja único (flipa device-verify/share/userMenu).
- [x] **f1-demote-oidc** (S) — `--color-oidc-bg` → `--color-info-blue` (botón SSO distinto de la marca, en el azul info único).
- [x] **f1-demote-purple** (M) — Familia `--color-purple-*` **eliminada** (0 consumidores) de variables.css y del fallback de dark.css.
- [~] **f1-demote-misc-blue** (M) — **Parcial:** share-link → info-text. _admin-blue y avatar-gradient (decorativos) los dejé — bajo valor, requerirían restyle del admin._
- [x] **f1-named-colors** (S) — `white/black` → `#ffffff/#000000`; confirmado **0 named colors** fuera de variables/themes.

### Unificación de marca en superficies externas (primera impresión) ✅
- [x] **f1-share-accent** (S) — Hecho vía el alias `--color-primary → --color-accent` (wordmark + spinner de share ahora naranjas).
- [x] **f1-device-accent** (S) — Acento vía alias **+ bug del Deny arreglado** (`--color-error-text` como fondo → `--color-danger-bg`).
- [x] **f1-auth-foreground** (S) — 3× `--color-danger-text` foreground → `--color-on-accent` en auth.css (+ 2× en device-verify).
- [x] **f1-share-logo** (S) — Tile de marca canónico (`.brand-mark` con el SVG real) añadido a share.html.
- [x] **f1-device-logo** (S) — Tile `.brand-mark` añadido a device-verify.html (SVG inline → no hizo falta icons.js).
- [x] **f1-share-emoji-expired** (S) — 🚫 `&#128683;` → SVG alert-circle (color danger).
- [x] **f1-share-emoji-file** (M) — 📄 `&#128196;` → SVG de documento (color accent).
- [~] **f1-share-card-icons** (S) — **Diferido:** los cards de folder/file los construye publicShare.js (JS, no HTML).
- [x] **f1-share-download-icon** (S) — SVG de descarga añadido al botón Download.
- [~] **f1-login-nojs** (M) — **Diferido:** requiere entender la lógica de paneles de auth.js (riesgo de mostrar panel equivocado).
- [x] **f1-share-nojs** (S) — `<noscript>` con mensaje de fallback añadido.

### Marca: componente canónico
- [x] **f1-about-real-mark** (S) — **`fa-cloud` genérico → marca SVG real** en el modal "Acerca de" (+ gradiente unificado a `--color-logo-gradient`).
- [x] *(nuevo)* **brandLogo.css** — Componente `.brand-mark` canónico creado (gradiente único + SVG), usado en share + device + base para el resto.
- [~] **f1-logo-sidebar/admin/profile/auth** (M) — **Diferido:** sus tiles existentes funcionan y ya usan tokens de gradiente; migrarlos al componente es bajo valor / más riesgo sin render.
- [~] **f1-settings-container** (M) — **Diferido:** unificar admin 1080 vs profile 720 es una decisión de layout (los dashboards suelen ir más anchos).
- [~] **f1-extract-toggle** (M) — **Diferido:** refactor de organización (el toggle solo vive en admin).

### Responsive: reflow del file manager ✅ (overrides solo-móvil → desktop intacto)
- [x] **f1-list-mobile** (L) — **Reflow robusto basado en flex** en `@media (max-width:640px)`: cada fila → línea compacta (icono+nombre … tamaño+acciones), header de columnas oculto. Flex ignora los tracks del grid → sin desalineación header/fila.
- [x] **f1-list-trash-mobile** (M) — Cubierto por el reflow universal (oculta `.path-cell` en móvil).
- [x] **f1-list-fav-recent** (M) — Cubierto por el reflow universal (mismo `.file-item`).
- [x] **f1-grid-density** (M) — `--grid-card-min` tokenizado (200px → 140px en móvil) + gap apretado.
- [x] **f1-actions-bar-responsive** (M) — `flex-wrap` + `height:auto` en móvil.
- [x] **f1-gutter-align** (S) — Token `--gutter` (24px) en topbar **y** content → comparten borde izquierdo; baja a 16px en móvil vía override de `:root` en media query.
- [x] **f1-vh-views** (M) — `100vh → 100dvh` en 6 vistas (music/auth/admin/profile/device/share).
- [~] **f1-mobile-first-sidebar / topbar** (M) — **Diferido:** invertir a mobile-first es refactor con riesgo de regresión desktop, valor de usuario bajo (ya funcionan).
- [~] **f1-sidebar-bp-sync** (M) — Diferido (requiere revisar el supuesto 768px en JS).
- [~] **f1-mq-named** (L) — Diferido (mismo bloqueo que **f0-breakpoints**: `@custom-media` no es nativo y dev sirve CSS crudo sin preprocesar → rompería las media queries en `cargo run`).
- [~] **f1-container-query-list** (L) — Diferido (el reflow flex ya logra el objetivo móvil; container query es mejora opcional).
- [~] **f1-sidebar-resizable** (L) — Diferido (feature: JS + localStorage).
- [~] **f1-density-toggle** (M) — Diferido (feature: UI + wiring de los tokens `--density-*` ya definidos).

### Rendimiento (build)
> **Corrección (2026-06):** SÍ existe un build pipeline en Rust — [`build.rs`](../build.rs) (release) copia `static/→static-dist/`, bundlea CSS/JS a `app.{hash}.css|js` (content-hash FNV-1a), minifica con **lightningcss** (CSS) y **oxc** (JS), reescribe `index.html` y actualiza `sw.js`. **Sin npm/devDependencies** — todo son crates Rust ya en `Cargo.toml`. Esto desbloquea trabajo antes diferido "por no haber build step", pero cada tarea tiene su propio bloqueo real (abajo), distinto de la falsa premisa anterior.
- [x] **f1-resource-hints** (M) — **Hecho (build.rs):** `rewrite_index_html` inyecta `<link rel="preload" as="style">` + `<link rel="modulepreload">` para los bundles hasheados, justo tras `<meta charset>` (charset queda primero). El `theme-init.js` clásico es render-blocking y retrasa el descubrimiento de los bundles; los hints arrancan ambas descargas en paralelo, ahorrando un RTT serializado. _Verificado: harness `rustc` aislado sobre el `index.html` real → 1 stylesheet + hints presentes; build release regenera `static-dist/index.html`._
- [x] **f1-inline-theme-init** (S) — **Hecho (build.rs):** `theme-init.js` (clásico, render-blocking, ~200 B) ahora se **inlinea minificado** en el `<head>` de index.html durante `rewrite_index_html` → una petición menos en el critical path en la primera carga (antes de tener SW). Debe correr antes del paint (evita flash de tema) y se mantiene en su posición. Las otras 4 páginas (login/admin/profile/share) conservan la referencia externa (archivo cacheado). Fallback al `<script src>` si la lectura falla. _Verificado en `static-dist/index.html`: 0 `<script src=…theme-init>`, 1 script inline; archivo externo intacto._
- [~] **f1-critical-css** (L) — **Diferido (razón real, no "build step"):** inline crítico exige extraer el subconjunto above-the-fold (necesita render headless / penthouse) y cargar el resto async sin FOUC → **requiere render-review**. Inlinear el bundle entero infla el HTML (que va por `include_str!` al binario) y mata el cache de CSS. No es seguro a ciegas.
- [~] **f1-code-split** (XL) — **Diferido (razón real):** el bundler de `build.rs` es un **concatenador del grafo completo** (un IIFE, elimina `import`/`export`). Code-splitting de verdad necesita emisión de chunks + dedup de deps compartidas en `build.rs` **y** refactor de los entry points (photos/music) a `import()` dinámico. **admin/profile ya están fuera del bundle** (páginas aparte, minificadas sueltas por `minify_tree_js`). Cambio grande y arriesgado, ROI bajo en una SPA de un solo shell.
- [x] **f1-thumb-content-type** (S) — **Hecho (backend Rust):** `thumbnail_content_type()` en [mime_detect.rs](../src/common/mime_detect.rs) (reusa `infer`, 0 deps nuevas) detecta el formato real por magic bytes; los 3 `image/jpeg` hardcodeados en [file_handler.rs](../src/interfaces/api/handlers/file_handler.rs#L386) → Content-Type correcto. El fast-path (PNG/GIF/WebP guardado tal cual) ya no se sirve como JPEG. _Verificado: fmt + clippy -D warnings + 15 tests._

---

## FASE 2 — Estados e accesibilidad (el salto a "profesional")

### Foco, disabled, loading ✅ — baseline global + matriz de `.btn`
- [x] **f2-focus-token** (S) — **[base/a11y.css](../static/css/base/a11y.css)**: `:focus-visible { outline: 2px solid var(--color-focus-ring); offset 2px }` global + `:focus:not(:focus-visible){outline:none}` (suprime el anillo en click, lo muestra con teclado).
- [x] **f2-focus-rollout** (L) — El baseline da anillo de teclado a **todo** elemento sin override propio (botones, nav, links, context-menu, chips…). Mouse users: 0 cambio; teclado: foco visible en toda la app.
- [x] **f2-remove-outline-important** (S) — Los 2 `outline:none !important` del languageSelector eliminados → el toggle de idioma recupera anillo de teclado.
- [x] **f2-btn-focus** (S) — `.btn:focus-visible` explícito (anillo crujiente sobre los gradientes).
- [x] **f2-btn-disabled** (M) — `.btn:disabled / [disabled] / .is-disabled`: dimmed, sin lift, `pointer-events:none`.
- [x] **f2-btn-loading** (M) — `.btn.is-loading / [aria-busy]`: oculta el label + spinner inline (`@keyframes spin` canónico + `--spin-duration`), bloquea interacción.
- [x] **f2-audit-outline-none** (L) — **Auditado regla por regla:** son **8 reglas** (no 24 — el conteo previo contaba ocurrencias e ignoraba el anillo), **todas en inputs de texto** y **todas con indicador visible** → **0 peladas/rotas**. 7 ya usan el patrón correcto (`outline:none` + anillo acento `box-shadow`), que para inputs **debe** mostrarse en `:focus` (ratón+teclado) — convertirlos a `:focus-visible` sería peor UX. La única inconsistente (`.music-shares-input`: borde-only a `--color-text`) **armonizada** al anillo acento estándar. _Verificado: barrido confirma 0 peladas; build 0 errores._
- [~] **f2-states-components** (L) — **Parcial:** el foco de todos los componentes ya lo cubre el baseline. Falta disabled/loading por-componente más allá de `.btn`.

### Contraste WCAG ✅ — audit completo: 0 fallos en light Y dark (verificado por script)
- [x] **f2-contrast-text** (M) — muted/subtle/faint light → `#5e6a78` (pasan AA sobre `#fff`, page `#f5f7fa` **y** muted `#f0f3f7`). _El audit pilló que page/muted bg fallaban (4.43/4.28) — corregido._
- [x] **f2-contrast-dark** (M) — muted/subtle/faint dark → `#9fadbe` (pasan AA sobre surface, page **y el hover `#334155`**, que fallaba a 4.04/3.50).
- [x] **f2-contrast-link** (M) — auth-toggle-link, about-link, language-option activa → `--color-accent-text`; share-link ya iba a info (Fase 1). _Iconos y estados hover/activos se dejan en accent vibrante (intencional)._
- [x] **f2-contrast-matrix** (L) — **Audit exhaustivo** (resuelve `light-dark()` + alias): cada par text×bg + semántico en ambos modos. accent-text light → `#cc3a16` (pasaba 4.45 sobre page → ahora ≥4.5). **Resultado: 0 fallos.**

### Teclado, landmarks y semántica ✅ — la nav ya es operable por teclado
- [x] **f2-nav-anchors** (M) — Los 8 `<div class=nav-item>` → **`<button class=nav-item type=button>`** (teclado nativo Enter/Space, 0 cambio en el JS de click) + resets de botón en `.nav-item` + iconos `aria-hidden`.
- [x] **f2-nav-aria-current** (S) — `aria-current="page"` en el item activo (en `setCurrentSection`, [navigation.js](../static/js/app/navigation.js)).
- [x] **f2-landmarks-index** (S) — `.nav-menu` → `<nav aria-label="Primary">`; `.content-area` → `role="main" id="main"`. _(`<header>` del topbar: opcional, omitido.)_
- [x] **f2-skip-link** (S) — `<a href="#main" class="skip-link">` como primer elemento del body + estilo en a11y.css (oculto hasta foco).
- [x] **f2-lang** (S) — `lang="en"` en `<html>` de index.html y login.html (fallback estático; languageSelector.js lo actualiza en runtime).
- [x] **f2-landmarks-login** (S) — `.auth-container` → `role="main"` (+ el h1 sr-only de Fase 1).
- [x] **f2-landmarks-secondary** (M) — `role="main"` en admin/profile/share/device-verify; nextcloud-* ya tenían `lang`.
- [~] **f2-roving-tabindex** (L) — **Diferido:** navegación por flechas en grid/lista es JS no trivial (gestión de tabindex móvil).

### Modales, live regions, formularios — núcleo de modales hecho
- [x] **f2-modal-semantics** (M) — `role="dialog"` + `aria-modal="true"` + `aria-labelledby` en input-modal y about-modal (id en su h2).
- [x] **f2-modal-focus-trap** (M) — **Focus-trap (Tab cicla dentro) + guardado/restauración de foco** al trigger en la clase Modal ([modal.js](../static/js/components/modal.js)). _Verificado con `node --check`._
- [~] **f2-modal-escape** (M) — input-modal ya tenía ESC + click-overlay (confirmado). _Los otros overlays (share/groups/lightbox) son separados → follow-up._
- [x] **f2-icon-button-names** (M) — `aria-label` + `aria-hidden` en los botones de icono sin nombre (notif-bell, user-avatar, modal-close); el resto ya tenía label.
- [x] **f2-theme-radio** (S) — El radiogroup ya mantiene `aria-checked` (userMenu.js:104); arrow-roving = polish opcional.
- [x] **f2-live-region** (M) — **Hecho:** [liveRegion.js](../static/js/core/liveRegion.js) crea una región `aria-live` global (polite + assertive, `.sr-only`, lazy); [uiNotifications.js](../static/js/app/uiNotifications.js) anuncia cada notificación (errores en `assertive`). El toast de errorBoundary ya tenía `role="alert"`. _`node --check` OK; bundle release verificado._
- [~] **f2-form-errors** (M) — **Diferido:** `aria-invalid`/`role=alert` + foco al primer inválido en auth.js.
- [~] **f2-touch-targets** (M) — **Diferido:** subir iconos a 44px es cambio visual de layout — mejor con la app delante.
- [~] **f2-lang-picker-keyboard** (M) — **Diferido:** combobox custom, JS no trivial.

### Movimiento y preferencias del sistema ✅ — media queries CSS (aditivas, riesgo cero)
- [x] **f2-reduced-motion** (S) — **Guard global `prefers-reduced-motion`** en a11y.css (animaciones/transiciones a ~0, sin scroll suave). Era 0 ocurrencias → ahora honra la preferencia del SO en toda la app.
- [x] **f2-prefers-contrast** (M) — Tier `prefers-contrast: more`: bordes más fuertes, muted/subtle/faint → secondary, anillo de foco 3px.
- [x] **f2-forced-colors** (S) — `forced-colors: active`: anillo de foco usa `Highlight` del sistema.

### Skeletons y breadcrumb (loading sin salto)
- [x] **f2-skeleton-component** (M) — **[skeleton.css](../static/css/components/skeleton.css) creado:** primitivo `.skeleton` reutilizable + `.skeleton-line/-row/-tile` (la row hereda las columnas de la lista). Honra reduced-motion.
- [x] **f2-files-skeleton** (M) — **Cableado:** [filesView.js](../static/js/app/filesView.js) reemplaza el spinner de carga por **skeleton cards (grid) / rows (list)** según `app.currentView` (8 placeholders, `role=status`/`aria-busy`, honra reduced-motion). Nuevos `.skeleton-card` (tile 4:3 + 2 líneas) y `.skeleton-icon` en [skeleton.css](../static/css/components/skeleton.css).
- [~] **f2-list-views-skeleton / f2-photos-skeleton / f2-search-skeleton** (S) — **Pendientes:** mismo patrón ya disponible; falta cablearlo en photos/search/otras vistas (la de archivos ya sirve de plantilla).
- [~] **f2-breadcrumb-collapse** (L) — **Diferido:** colapso con menú `…` requiere medir ancho en JS.
- [~] **f2-lightbox-dialog** (M) — **Diferido:** focus-trap en photosLightbox.js (reutilizable del patrón de modal.js).
- [~] **f2-photo-tile-keyboard** (M) — **Diferido:** tiles enfocables/operables (JS).

### Superficies externas: estados — login cubierto
- [x] **f2-ext-focus** (M) — Cubierto por el baseline global `:focus-visible` de a11y.css (las 3 páginas cargan main.css); inputs ya tienen anillo border/box-shadow.
- [x] **f2-login-submit-loading** (M) — Login: `.is-loading`+`aria-busy`+disabled en submit, reset en `finally` ([auth.js](../static/js/features/auth/auth.js)) + CSS de spinner en `.auth-button`. _`node --check` OK._
- [x] **f2-ext-aria** (M) — `autocomplete` en login (`username`/`current-password`/`email`). _register/admin-setup: follow-up._
- [~] **f2-share-loading / f2-device-loading** (M) — **Diferidos:** mismo patrón, falta cablearlo en publicShare.js / device-verify.js.
- [~] **f2-error-icons** (L) — **Diferido:** componente de banner con icono líder (CSS+HTML).
- [x] **f2-consolidate-ctas** (M) — **Hecho (tras revisar capturas):** el login tenía **dos botones primarios gradiente** compitiendo; el de magic-link pasa a **secundario** (`.auth-button-secondary`, tinte/ghost) → una sola acción primaria por pantalla.
- [x] **f2-progressive-magic-link** (M) — **Hecho:** el formulario de magic-link se **colapsa** tras un link discreto ("¿Sin contraseña? Recíbelo por correo", `#magic-link-toggle` con `aria-expanded`/`aria-controls`); al abrir, foco al email. Pantalla más tensa e intencional. _`node --check` OK._

### Pulido premium de superficies auth (tras capturas) ✅ — login + setup wizard
- [x] **f2-auth-brand-oxi** (S) — Wordmark con el **lockup "Oxi" en acento** (`.brand-oxi`, gradiente de marca con `background-clip:text`) en los 4 paneles — el tratamiento *ownable* del DESIGN-SYSTEM §3.
- [x] **f2-auth-field-icons** (M) — **Iconos guía** (user/mail/lock) en los 11 inputs vía **CSS `mask`** (data-URI sin color → token-safe; color por `--color-text-muted`, acento en `:focus-within`). 0 SVG inline en HTML, solo una clase modificadora.
- [x] **f2-auth-pw-toggle** (M) — **Show/hide de contraseña** (ojo) en los 5 campos password (`[data-pw-toggle]`, `aria-pressed`/`aria-label` localizada, icono eye/eye-off por máscara). _`node --check` OK._
- [x] **f2-auth-pw-match** (M) — **Indicador de coincidencia en vivo** bajo "Confirmar contraseña" (register + admin): check verde / cruz roja + texto (`auth.passwordsMatch` / `auth.passwords_mismatch`), `aria-live="polite"`.
- [x] **f2-auth-stepper-connector** (S) — Stepper del setup → **grid 3-col con línea-track conectora** + halo del paso activo (antes los círculos flotaban sueltos).
- [x] **f2-auth-input-definition** (S) — Borde de input a `--color-border-medium` (definición), placeholder a `--color-text-muted` (legibilidad); foco con anillo acento ya existía.
- [x] **f2-auth-divider-press** (S) — Divisor "o" → **uppercase/tracked/muted**; press táctil del botón (`translateY(1px)`); glow del tile suavizado (spread negativo). 2 claves i18n nuevas (`magicLinkToggle`, `passwordsMatch`) en **los 16 locales** (es/en propias) → `check-locales` verde.
- [x] **f2-auth-ambient** (M) — **Fondo atmosférico premium** centralizado en `--brand-ambient` (spotlight con `bg-surface` + mesh de marca) + `--brand-grain` (ruido fractal desaturado, `mix-blend overlay`, token-safe). Aplicado a login/share/device (mata la duplicación 3×); grano por ahora solo en login. _lightningcss minifica el data-URI sin fallar._
- [x] **f2-auth-readonly** (S) — Campo `[readonly]` (admin user) ya no se atenúa como placeholder: texto full-strength + fill "locked" (`--color-bg-input-alt`).

### Pulido tras 2.ª ronda de capturas (app completa) ✅
- [x] **f3-share-i18n** (M) — **El modal de Compartir salía en inglés** (faltaban 17 claves `share.*`/`actions.*`; el JS ya usaba `i18n.t` con fallback). Añadidas a **los 16 locales** (es propio + inglés de fallback) → modal en español (Compartir:, Personas, Enlaces públicos, Puede ver, Sin caducidad, Aplicar, Añadir…). `check-locales` verde.
- [x] **f1-nav-active-unify** (S) — Iconos del sidebar: **estado activo siempre en acento** (antes el icono activo cambiaba de color por `nth-child` → cal-7/8/9…). Decisión del usuario: **mantener colores en reposo** (tipo Notion), unificar solo el activo.
- [~] **f1-nav-avatar** (S) — **Mantenido a propósito:** el color del avatar es **por-usuario** (hash de `userId` → `uv-color-N`, patrón Slack/Google = identidad visual), no un bug. No se fuerza a acento.

### Correcciones del dashboard de archivos (3.ª ronda de capturas) ✅
- [x] **f-bug-date-1970** (S) — **BUG REAL:** la fecha "Modificado" salía **21/1/1970**. Causa: [resourceList.js](../static/js/components/resourceList.js#L623) hacía `formatDateTime(new Date(dateVal))` — `modified_at` es Unix en **segundos** (~1.7e9), `new Date()` lo tomaba como **ms** → epoch+21d. Fix: pasar `dateVal` directo (el formatter ya convierte seg→ms). _Verificado: OLD→1970, NEW→2025; `node --check` OK. Otros renders sin el antipatrón; `profile.js` usa ISO (no afectado)._
- [x] **f-grid-thumbnails** (M) — Miniaturas de la rejilla: `.file-icon` era fijo 100×70 (imagen pequeña, letterbox) → ahora **full-width con `aspect-ratio: 4/3`** + el `.file-thumb` (object-fit:cover) **llena edge-to-edge** (rejilla uniforme tipo Drive); iconos de no-imagen centrados sobre fondo placeholder.
- [x] **f-grid-name-ellipsis** (S) — El ellipsis estaba en `.name-cell` pero el texto va en un `span` (block) → no aplicaba. Movido al `span` (`nowrap/overflow/ellipsis`) + **`title`** en los nombres (folder+file) para tooltip al truncar.
- [x] **f-grid-meta** (S) — El label redundante **"Imagen"** bajo cada card (ya se ve por la miniatura) → reemplazado por el **tamaño** (`size-cell`) como metadato secundario. _(Carpetas muestran "--" en rejilla — edge case menor.)_
- [x] **f-files-skeleton** (M) — Skeletons cableados en la carga de archivos (ver `f2-files-skeleton`) → sin flash de lista vacía mientras cargan miniaturas/datos.
- [x] **f-breadcrumb-home** (S) — El home folder mostraba su nombre técnico "My Folder - admin" → ahora muestra la etiqueta amigable **"Archivos"** (reutiliza `nav.files`, ya en los 16 locales; el nombre real queda como `title`).
### Card premium de la rejilla (4.ª ronda — "producto final") ✅
- [x] **f-card-premium** (M) — Redesign de la grid card: **borde hairline 1px → acento al hover** (antes 2px gris), **hover signature** (`translateY(-3px)` + sombra suave grande + **miniatura `scale(1.04)`** dentro del `overflow:hidden`), **anillo interior** en la miniatura (`inset 0 0 0 1px`), tipografía `text-sm`, transiciones con tokens de motion.
- [x] **f-card-overlay-actions** (M) — Estrella/kebab/checkbox/badge-shared **ya no flotan en la esquina de la card** → reposicionados **sobre la miniatura** con **scrim frosted** (`--color-scrim-control` nuevo + `backdrop-filter`): checkbox top-left, estrella+kebab top-right, badge-shared top-left bajo el checkbox.
- [x] **f-dropzone-explicit** (M) — El `#dropzone` (mostrado al arrastrar archivos vía ui.js) pasa de caja inline pequeña a **overlay full-area** sobre el área de contenido (`position:fixed`, offset del sidebar, borde punteado, backdrop frosted, icono acento grande, "Arrastra archivos aquí"). `pointer-events:none` en hijos evita el flicker de dragleave.
- [~] **f-grid-share-download-btns** (S) — **Diferido (opcional):** botones dedicados de compartir/descargar al hover vía `customActions` — pero ese mecanismo es **global** (afecta lista + carpetas), necesita scoping grid-only/file-only. El kebab ya cubre la función (el usuario lo marcó opcional).
- [—] **f-grid-select** — **Ya existía:** selección por checkbox al hover en rejilla.
- [x] **f-bug-tooltip-orphan** (S) — **BUG REAL:** el tooltip del email del avatar se quedaba pegado al salir el ratón. Causa: `_mountAvatarVignettes` re-renderiza la vignette con `replaceChildren` (p.ej. al actualizarse el storage), pero el popover de `attachTooltip` está portalado a `document.body` y nadie llamaba a su cleanup → quedaba huérfano y visible. Fix: WeakMap de cleanups + `disposeVignette()` que se llama sobre el hijo viejo antes de re-montar. _`node --check` OK._

### Pulido del menú de usuario (desplegable) ✅
- [x] **f-menu-no-avatar-tooltip** (S) — Opción `noTooltip` en la vignette → el avatar propio del topbar **ya no muestra el tooltip del email** (redundante con el header del menú + solapaba la campana). Mata la causa del bug y el "tell" de redundancia.
- [x] **f-menu-appearance-icon** (S) — Icono líder de "Apariencia" `fa-moon` → **`fa-adjust`** (medio-círculo de tema/contraste; FA5+FA6) — neutro, ya no implica modo oscuro.
- [x] **f-menu-row-focus** (S) — `:focus-visible` en las filas del menú (mismo tinte que hover) → navegación por teclado clara.
- [—] **f-menu-segmented-aria** — **Ya existía:** los 3 botones del segmented (sol/escritorio/luna) ya tenían `aria-label` i18n (light/auto/dark).
- [~] **f-menu-role-chip** (S) — **Diferido:** mover el badge "Admin" junto al nombre en el header requiere reestructurar la vignette (generada por JS). Bajo valor / más riesgo.
- [~] **f-menu-shortcut-hints** (S) — **Diferido:** mostrar atajos (⌘,) sin que existan los atajos reales sería una afordancia falsa; primero habría que cablear los shortcuts.

### Pulido premium de la list view (tras captura) ✅
- [x] **f-list-thumb-ring** (S) — Miniaturas de lista a 40×40 + **anillo interior** (`inset 0 0 0 1px`) → consistente con la rejilla premium (no sangran sobre la fila).
- [x] **f-list-row-accent** (S) — **Barra-acento izquierda** en hover/selected (`inset 3px 0 0 var(--color-accent)`, sin layout shift) + transición → énfasis "esta fila".
- [x] **f-list-hover-actions** (S) — Kebab **revelado al hover** (`@media (hover:hover)`; en táctil sigue visible; visible en `:focus-within` para teclado) → filas más limpias.
### Última milla del login (tras captura) ✅
- [x] **f-login-bg-stronger** (S) — **El fondo se veía blanco/amateur** → `--brand-ambient` reforzado (glow 0.13→0.20 + **4 blobs cálidos** que llenan el lienzo, no solo esquinas) + grano a 0.6 → canvas dimensional, ya no plano.
- [x] **f-login-autofocus** (S) — **Autofocus** en el campo usuario al mostrar el panel (los paneles arrancan ocultos → focus por JS, no atributo) — cursor listo al cargar y al volver desde registro.
- [x] **f-login-capslock** (S) — **Aviso "Bloq Mayús activado"** bajo los campos de contraseña (login/register/admin) vía `getModifierState('CapsLock')` — evita el login fallido. Clave `auth.capsLock` en los 16 locales.
- [—] **f-login-sso** — **Ya existía:** botón SSO condicional (`configureOidcLoginUI` cuando hay OIDC).
- [~] **f-login-remember** (S) — **Diferido:** "Recordarme" necesita **soporte backend** (no hay param remember/persistent en `/api/auth/login`; sería un checkbox falso). Requiere TTL de refresh-token configurable.
- [—] **f-login-forgot** — **N/A:** no hay self-service password-reset (solo `admin_reset_password`). El **magic-link ES la vía de recuperación** sin contraseña — un "¿olvidaste?" apuntaría a nada.

- [x] **f-list-sortable-headers** (M) — **Hecho (opción A, sort plano tipo Drive):** las cabeceras Nombre/Tipo/Tamaño/Modificado son **clicables** → flat sort asc/desc (sin swimlanes), con **flecha** en la columna activa que invierte al re-clicar. Nuevo path en `filesView` (`_sortField` que sobreescribe el `order_by` del group-by y fuerza `_groupBy=''`; `setSortField` + getter `currentSort`); el header se sincroniza desde `_loadPage` (header click / toolbar group-by / dirección). `aria-label` localizado con dirección (`sort.asc/desc` en los 16 locales). _Verificado: state machine (click size→asc, re-click→desc, group type→limpia flat), node --check, build 0 errores._
  - _Nota: el sort plano usa `order_by` server-side (el agrupado en swimlanes del toolbar sigue disponible aparte). El botón de dirección del toolbar ahora también re-sincroniza la flecha vía `_loadPage`._

### Panel de notificaciones (tras captura) ✅
- [x] **f-notif-i18n** (L) — **El idioma mezclado del panel** (notificaciones en inglés en app española): **52 `showNotification('inglés')` hardcoded** en 8 archivos → convertidas a `i18n.t` vía **workflow multi-agente** (1 agente/archivo, en paralelo) + manejo de interpolación (`${var}`→`{{param}}`). **67 claves `notif.*` nuevas** traducidas al español y añadidas a **los 16 locales** (es propio + inglés de fallback). _Verificado: node --check los 8, 0 hardcoded restantes, todas las claves referenciadas cubiertas, `check-locales` verde, build 0 errores._
- [x] **f-notif-scrollbar** (S) — Scrollbar del panel: del **global naranja grueso** a uno **fino gris (6px)** específico (`.notif-panel-body`).
- [~] **f-notif-relative-time / f-notif-fav-icon / f-notif-read-state** (S) — **Pendientes (menores):** hora relativa ("hace 5 min" con `formatRelativeTime`, requiere guardar timestamp real), color del icono de favorito (verde→dorado), y estado leído/no-leído.

### Bug del visor de imágenes (tras capturas) ✅
- [x] **f-bug-viewer-overflow** (S) — **BUG REAL:** las imágenes anchas se **desbordaban** del visor (scroll horizontal + barra naranja gruesa) → parecía que "se repetían" al scrollear. Causa: `.inline-viewer-image` es flex item y el `min-width: auto` por defecto de flexbox **ignora el `max-width: 100%`** (no encoge). Fix: `min-width: 0; min-height: 0` → la imagen ajusta entera, sin scroll. + **scrollbar fino/sutil** en el visor (para el caso de zoom; reemplaza el global acento grueso). _Build 0 errores._

### Rendimiento (foco en shell)
- [~] **f2-icon-foit** (L) — **Diferido:** eliminar el FOIT de iconos del shell requiere inlinear iconos críticos o un build — relacionado con el grupo de build/perf pendiente.

---

## FASE 3 — Pulido (de "sólido" a "premium")

### Empty states con CTA + ilustración — CSS compartido mejora TODAS
- [x] **f3-empty-component** (M) — `.empty-state` mejorado ([content.css](../static/css/layout/content.css)): icono **accent** (era gris apagado), primer `<p>` como título (lg/semibold/heading), desc muted con medida, spacing de CTA. Como todas las vistas comparten `.empty-state`/`.empty-state-icon`, **mejora las 7 a la vez** sin tocar su JS.
- [x] **f3-empty-files** (M) — CTA **"Upload files"** cableado en `showEmptyList` (ui.js) → abre el file picker. _`node --check` OK._
- [~] **f3-empty-photos / f3-empty-search / f3-empty-trash / f3-empty-rest** — **Visualmente mejoradas** por el CSS compartido; el **CTA propio** de cada una (subir fotos / limpiar búsqueda / etc.) queda de follow-up (JS por vista). Ilustraciones SVG bespoke → Fase 4.

### Fotos y lightbox (nivel Google Photos)
- [x] **f3-photo-placeholder** (M) — `decoding="async"` + fondo placeholder muted en `.photo-tile` + **fade-in al cargar** (`_fadeInTiles` chequea `img.complete`, listeners `once`) → se acaba el "pop-in". _`node --check` OK._
- [x] **f3-lightbox-preload** (M) — `_preloadNeighbors()` precarga el thumbnail prev/next en cada `_show` → navegación instantánea.
- [x] **f3-lightbox-spinner** (S) — Ya estaba: spinner girando (`.photos-loading i` con `spin`) + fallback "Failed to load" en error (verificado).
- [~] **f3-justified-grid** (L) — **Diferido:** grid justificado necesita aspect-ratios + algoritmo de packing (feature grande).
- [~] **f3-lightbox-zoom** (L) — **Diferido:** zoom/pan/doble-tap (gestos, JS no trivial).
- [~] **f3-lightbox-swipe** (L) — **Diferido:** swipe táctil con rebote (gestos).

### Refinamiento visual y movimiento — wins CSS aditivos hechos
- [x] **f3-listrow-hover** (S) — Transición en `.file-item` (bg/border) **rutada por `--motion-fast`+`--ease-standard`** (curva de desaceleración) → las filas ya no "snapean". El grid mantiene su transición de transform.
- [x] **f3-text-balance** (S) — `text-wrap: balance` en headings + `pretty` en prosa (empty-state, about, auth) en [typography.css](../static/css/base/typography.css).
- [x] **f3-tabular-nums** (M) — `font-variant-numeric: tabular-nums` en storage/badge/stat-value vía una regla central (size-cell y música ya lo tenían).
- [~] **f3-prose-measure** (M) — Parcial: `text-wrap: pretty` aplicado; la medida (max-width) ya la acotan los contenedores estrechos + `.empty-state p` (65ch explícito → Fase 1 via `.prose`).
- [~] **f3-filetype-glyphs / f3-dragdrop / f3-search-results / f3-toast-exit / f3-overlay-travel / f3-spinner-primitive / f3-shadow-adoption / f3-hover-active-press / f3-dark-elevation / f3-display-clamp** — **Diferidos:** todos **cambian valores visuales existentes** de forma amplia (sombras, easing, glifos, elevación) → necesitan verse renderizados antes. _El barrido de motion (rutar transiciones por `--ease-*`) y el de sombras (→ tokens `--shadow-*`) son los de mayor impacto pendiente._

### Login/share atmosféricos + marca — primera impresión CSS hecha
- [x] **f3-login-bg** (L) — Fondo del login: gris plano → **gradiente atmosférico** (dos radiales de brand-glow sutil sobre `--color-bg-page`). En dark, el glow cálido sobre el navy se ve premium.
- [x] **f3-share-bg** (M) — Mismo gradiente en share **y** device-verify (antes `bg-hover` plano) → primera impresión coherente.
- [x] **f3-card-elevation-ext** (M) — Las 3 tarjetas externas (auth-panel/share-card/dag-card) unificadas a `box-shadow: var(--shadow-xl)` + `border-radius: var(--radius-3xl)`.
- [x] **f3-share-trust-footer** (M) — Footer estático "🔒 Secured by OxiCloud" en la página de share. _Expiry/password badge dinámicos → follow-up JS._
- [x] **f3-logo-motion** (S) — El hover del logo ya respeta reduced-motion (vía el guard global de a11y.css).
- [~] **f3-share-attribution** (M+S) — **Diferido:** "compartido por" requiere exponerlo en la API (backend Rust) + render.
- [~] **f3-share-preview** (L) — **Diferido:** preview inline de archivo (JS + thumbnail).
- [~] **f3-device-code-segments** (M) — **Diferido:** input OTP segmentado (JS no trivial).
- [~] **f3-share-empty-error** (M) — **Diferido:** mejorar empty/error de la galería (JS/CSS).
- [~] **f3-brand-voice / f3-brand-voice-i18n** (M) — **Diferidos:** definir voz/tagline + reescribir "Acerca de" + propagar a 16 locales (contenido, decisión de marca).

### Marca: favicon / PWA / OG — wiring hecho (faltan rásters que no puedo generar)
- [x] **f3-pwa-manifest** (M) — [manifest.webmanifest](../static/manifest.webmanifest) + **iconos PNG maskable 192/512** rasterizados (vía `qlmanage` de macOS) → PWA instalable en todas las plataformas. Mantiene los SVG (`any`) + theme-color light/dark.
- [x] **f3-favicon-set** (M) — `apple-touch-icon` ahora apunta a **PNG 180×180** rasterizado (iOS ya muestra el icono al "añadir a inicio"; antes ignoraba el SVG). + SVG + favicon.ico + mask-icon en index/login/share.
- [x] **f3-og-image** (M) — **`og-image.png` 1200×630** rasterizado (recortado del thumbnail cuadrado de qlmanage); `og:image`/`twitter:image` apuntan al PNG en index/login/share → **previews reales** al compartir (las redes ignoraban el SVG).
- **✅ RESUELTO (antes el gap nº1 del audit):** los 3 PNG se generaron con `qlmanage -t -s N` (Quick Look rasteriza SVG en macOS) + `sips` para recortar el OG. Dimensiones exactas verificadas (180²/192²/512² + 1200×630), copiados a `static/logo/`, refs actualizadas. _iOS/redes/PWA ya no rotos._

### Responsive: pulido — CSS + a11y del drawer hecho
- [x] **f3-mobile-touch** (M) — `@media (max-width:768px)`: **≥44px** en toggle/search/notif/avatar, nav-items y `min-height:48px` en filas de lista (a11y.css).
- [x] **f3-drawer-focus-trap** (M) — Toggle con `aria-controls`+`aria-expanded` (sincronizado en open/close); foco al primer nav-item al abrir y **restaurado al toggle** al cerrar; **trap de Tab** dentro del drawer abierto (navigation.js). _`node --check` OK. `inert` de fondo omitido (el toggle vive en el fondo) — el overlay + trap lo cubren._
- [x] **f3-drawer-motion** (S) — Transición del sidebar → `var(--motion-slow) var(--ease-emphasized)` (curva de desaceleración, value-preserving 0.3s).
- [x] **f3-scrollbar-gutter** (S) — En móvil `scrollbar-gutter: auto` (sin gutter fantasma con scrollbars de overlay).
- [~] **f3-rtl-breakpoints** (M) — **Diferido:** verificación de paridad RTL en cada breakpoint → necesita render.
- [~] **f3-list-secondary-line** (M) — **Diferido:** línea secundaria (tamaño·fecha) en la fila móvil → restructura que conviene ver renderizada.

### Rendimiento (assets)
- [x] **f3-srcset-photos** (M) — Tiles con `srcset` (icon 150w / preview 400w / large 800w) + `sizes` → sirve la resolución justa (icon en móvil denso, large en retina). CLS ya lo cubre `aspect-ratio:1`. _`node --check` OK._
- [x] **f3-sw-strategy** (L) — **Navigation preload** añadido (enable + uso de `preloadResponse`) + bump de caché v27. El resto ya existía (precache versionado, `skipWaiting`, `clients.claim`, network-first HTML, stale-while-revalidate assets).
- [~] **f3-thumb-webp-avif** (L) — **Diferido:** WebP **lossy** (el que ahorra) necesita el crate `webp`/libwebp (dep nueva); el `image-webp` presente solo hace decode/lossless (no ayuda en fotos). AVIF necesita `ravif`. Decisión de dependencia.
- [~] **f3-lcp-preload** (M) — **Diferido:** en Photos/share la URL del LCP es **dinámica** (se conoce tras fetch JS) → no se puede `<link rel=preload>` estático sin lógica extra.
- [~] **f3-dead-css** (L) — **Diferido:** barrido de CSS muerto es arriesgado sin coverage tooling (clases generadas por JS → falsos positivos).

---

## FASE 4 — Clase mundial + guardarraíles (alcanzar y SOSTENER el 10)

### Interacciones de producto premium
- [x] **f4-command-palette** (L) — **Command palette global (Cmd/Ctrl+K)** ([commandPalette.js](../static/js/app/commandPalette.js) + [css](../static/css/components/commandPalette.css)): navegación a las 8 secciones + acciones (upload/profile/about/logout/admin), filtro por substring, ↑/↓/Enter/Esc, focus restaurado. **Ejecuta clicando los controles reales** → no se desincroniza. _`node --check` OK._
- [x] **f4-detail-polish** (S) — `::selection` con tinte de marca + `caret-color` acento (scrollbar ya era custom).
- [~] **f4-shortcut-grammar / f4-shortcut-help** (M/S) — Cmd+K ya es un atajo global; el sistema completo + overlay `?` quedan de follow-up.
- [~] **f4-view-transitions** (M) — **Diferido (doable):** envolver los cambios de sección en `document.startViewTransition()` (progresivo) — conviene verlo.
- [~] **f4-undo-destructive / f4-optimistic-everywhere / f4-dnd-finesse / f4-bulk-progress / f4-delight / f4-lightbox-filmstrip / f4-lightbox-shared-element / f4-empty-illustrations / f4-btn-signature / f4-high-dpi** — **Diferidos:** features grandes (undo/optimista/bulk), gestos/transiciones complejas (filmstrip/shared-element/dnd), o pulido que conviene ver renderizado.

### Internacionalización de verdad (16 locales) — Intl (sin deps)
- [x] **f4-i18n-format** (M) — `formatFileSize` → `Intl.NumberFormat` (separadores locale); `formatDateTime`/`formatDateShort` → pasan el **locale de la app** (antes el del navegador) en [formatters.js](../static/js/core/formatters.js).
- [x] **f4-i18n-relative-time** (S) — `formatRelativeTime()` con `Intl.RelativeTimeFormat` (CLDR, localizado) + **consolidados** los 2 `timeAgo` duplicados de admin/profile a usarlo (manteniendo el caso "never"). _`node --check` OK._
- [x] **f4-i18n-completeness-ci** (S) — [scripts/check-locales.mjs](../scripts/check-locales.mjs): claves faltantes/sobrantes + integridad de `{placeholders}`. Wireado en `just frontend-check`. **nl.json corregido → la suite pasa ✓** (los 6 mismatches igualados a en per-clave). _Hallazgo aparte: en.json mezcla `{{}}` y `{}` single en algunas claves (`actions.upload.complete`, `groups.member_count_other`) → bug latente de interpolación, consistente en los 16 → tarea i18n aparte._
- [~] **f4-i18n-plural** (L) — **Diferido:** `Intl.PluralRules` sin valor sin las formas plurales traducidas por locale (datos i18n).
- [~] **f4-i18n-collation** (M) — **Diferido:** no se encontró sort de nombres cliente (probablemente server-side).
- [~] **f4-i18n-bidi** (M) — **Diferido:** `unicode-bidi: isolate`/`dir="auto"` en nombres de usuario (CSS/JS contenido, follow-up).
- [~] **f4-i18n-rtl-logical** (XL) — **Diferido:** barrido físico→lógico (margin-left→margin-inline-start…) en toda la CSS.

### Offline, errores, onboarding, voz
- [x] **f4-error-boundary** (M) — [errorBoundary.js](../static/js/core/errorBoundary.js): `window.error` + `unhandledrejection` → toast throttled (`.error-toast`) + log a consola. Dependency-free, cargado primero. _`node --check` OK._
- [x] **f4-print** (M) — `@media print` en a11y.css: oculta chrome (sidebar/topbar/actions/cmdk) y limpia sombras para imprimir el contenido.
- [~] **f4-offline-shell / f4-offline-queue** (L/XL) — **Diferidos:** banner online/offline + cola durable de mutaciones (feature grande, termina el stub de background-sync del sw).
- [~] **f4-error-quota / f4-onboarding / f4-microcopy** (M/L) — **Diferidos:** estados de quota, primer-uso, y sistema de microcopy (contenido/feature).
- [x] **f4-live-region-async** (M) — **Hecho** (ver `f2-live-region`): región `aria-live` global que anuncia las notificaciones asíncronas.
- [~] **f4-pwa-install** (M) — **Diferido (doable):** prompt `beforeinstallprompt` + botón de instalar (tiene componente visual → mejor decidir ubicación con la app delante).
- [~] **f4-sr-flow-pass** (L) — **Diferido:** test manual con lector de pantalla (no automatizable aquí).

### Design system documentado (la fuente de verdad)
- [x] **f4-token-docs** (L) — [scripts/gen-token-docs.mjs](../scripts/gen-token-docs.mjs) → [docs/TOKENS.md](TOKENS.md) (**430 tokens, 20 grupos**, auto-generado).
- [x] **f4-a11y-docs** (S) — Sección de accesibilidad codificada en [docs/DESIGN-SYSTEM.md](DESIGN-SYSTEM.md) §2.
- [x] **f4-brand-guide** (M) — Mini-guía de marca (mark/wordmark/accent/clear-space/do-don'ts/maskable/OG) en DESIGN-SYSTEM.md §3.
- [~] **f4-perf-docs** (M) — **Parcial:** los guardarraíles documentados; falta la arquitectura de build/perf (depende de formalizar el `static-dist`).
- [~] **f4-gallery-page / f4-ds-site / f4-gallery-vr-axe** (XL/L/M) — **Diferidos:** styleguide vivo + snapshot/axe → necesitan tooling/build.
- [x] **f4-brand-drift-ci** (M) — **Hecho:** [check-brand-drift.mjs](../scripts/check-brand-drift.mjs) bloquea cambios silenciosos del logo (hash sha256 de `logo-plain.svg`) y del gradiente (`--color-logo-gradient`) contra un baseline bloqueado; en `just frontend-check`. **Pasa ✓**.
- [~] **f4-token-parity** (M) — **Diferido:** contrato de paridad con assets de diseño exportados (necesita los assets de diseño como fuente).
- [~] **f4-variable-font** (XL) — **Diferido:** evaluar/adoptar fuente variable self-hosted (decisión + asset).
- [~] **f4-opentype** (S) — **Diferido:** features OpenType (limitado en system stack; útil con fuente variable).

### Guardarraíles de CI — los sin-deps entregados y PASANDO
- [x] **f4-contrast-ci** (L) — [scripts/check-contrast.mjs](../scripts/check-contrast.mjs): resuelve `light-dark()`+alias y **falla si cualquier par texto/fondo o semántico baja de 4.5:1** en light o dark. **Pasa ✓**.
- [x] **f4-heading-order-ci** (M) — [scripts/check-headings.mjs](../scripts/check-headings.mjs): exige ≥1 `h1` y sin saltos de nivel por página. **Pasa ✓**.
- [x] **f4-dead-token-sweep** (M) — [scripts/check-dead-tokens.mjs](../scripts/check-dead-tokens.mjs): reporta tokens definidos sin `var()` (informativo, excluye prefijos dinámicos). **Encontró 96 candidatos** a podar.
- [x] **f4-ci-single-gate** (M) — Recipe `just frontend-check` que corre los 4 scripts (contrast/headings/locales/dead-tokens). _Falta añadir stylelint/biome/tsc cuando haya `node_modules`._
- [~] **f4-stylelint-scales / f4-lint-focus / f4-lint-breakpoints** (L/M) — **Pendiente:** reglas escribibles, pero **activarlas rompe CI hasta migrar los 184 px off-grid** (f1-snap-fractional) — hacerlo después de esa migración.
- [~] **f4-axe-ci / f4-keyboard-ci / f4-visual-regression / f4-dark-parity / f4-crossbrowser / f4-lighthouse-ci** — **Diferidos:** necesitan Playwright/axe/Lighthouse (**deps + browsers**, sin `node_modules` aquí). Decisión de tooling.
- [x] **f4-brand-drift-ci** (M) — **Hecho** (consolidado arriba): [check-brand-drift.mjs](../scripts/check-brand-drift.mjs) en `just frontend-check`.

> _Nota: este grupo se consolidó arriba (sección «Design system documentado»). El
> bloque duplicado de planificación original se eliminó para no inflar el conteo de
> pendientes — `f4-token-docs`/`a11y-docs`/`brand-guide` están hechos; el resto
> (gallery-page/ds-site/gallery-vr-axe/token-parity/variable-font/opentype/perf-docs)
> está diferido con su razón en la sección de arriba._

---

## Camino crítico (el orden que de verdad importa)

```
f0-stylelint-typo ─┐ (sin esto, nada se lintea)
f0-space/radius/type/motion/elevation/zindex ─┬─► f1-mig-* (migración) ─► f2-focus/contrast/nav ─► f4-stylelint/axe/contrast-ci
f0-oklch-ramps ────► f0-semantic-* ────────────┴─► f1-demote-*/fileicon-* (paleta)
f0-brand-mark ─────► f1-logo-* ───────────────────► f1-about-real-mark
f0-perf-baseline ──► f1-critical-css ─────────────► f4-lighthouse-ci
```

> **Nota de constraint (CLAUDE.md) — actualizada 2026-06:** el proyecto **SÍ tiene build pipeline**
> ([`build.rs`](../build.rs): bundling + content-hash + minify vía crates Rust lightningcss/oxc,
> **sin npm**). Lo que de verdad sigue necesitando devDependencies npm / navegadores es solo el
> **tooling de test de UI**: Playwright (keyboard/visual-regression/cross-browser), axe-core
> (a11y automatizada) y Lighthouse-CI (presupuestos de perf). WebP lossy necesita el crate `webp`.
> El resto de "diferidos por build" en realidad están bloqueados por: render-review (critical-CSS),
> complejidad/ROI (code-split), o el servido de CSS crudo en dev (`@custom-media`). CLAUDE.md
> prohíbe "npm deps no listadas" → el tooling de test necesita aprobación explícita; el riesgo está
> acotado a `devDependencies` (no toca el runtime ni el binario).

## Resumen por área (de la auditoría)

| Área | Nota actual | Tareas |
|------|:-----------:|:------:|
| Tokens / fundamentos | 3.0 | 29 |
| Tipografía | 3.0 | 43 |
| Color / paleta | 4.0 | 32 |
| Accesibilidad | 3.0 | 35 |
| Layout / responsive | 4.5 | 34 |
| Componentes / movimiento | 4.5 | 39 |
| Auth / primera impresión | 4.5 | 41 |
| Vistas núcleo | 4.0 | 41 |
| Marca / cohesión | 3.5 | 25 |
| Rendimiento | 6.5 | 15 |
| Tooling / CI / docs | 5.5 | 36 |
| Completitud (lo que faltaba) | — | 32 |
| **Total** | **4.8** | **402** |
