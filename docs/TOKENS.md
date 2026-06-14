# Design tokens

> Auto-generated from `static/css/base/variables.css` by `scripts/gen-token-docs.mjs`.
> Do not edit by hand — re-run the generator after changing tokens.

**430 tokens** across 20 groups.


## Spacing (4px grid)

| Token | Value |
| --- | --- |
| `--space-0` | `0` |
| `--space-px` | `1px` |
| `--space-0-5` | `2px` |
| `--space-1` | `4px` |
| `--space-1-5` | `6px` |
| `--space-2` | `8px` |
| `--space-2-5` | `10px` |
| `--space-3` | `12px` |
| `--space-3-5` | `14px` |
| `--space-4` | `16px` |
| `--space-5` | `20px` |
| `--space-6` | `24px` |
| `--space-7` | `28px` |
| `--space-8` | `32px` |
| `--space-9` | `36px` |
| `--space-10` | `40px` |
| `--space-11` | `44px` |
| `--space-12` | `48px` |
| `--space-14` | `56px` |
| `--space-16` | `64px` |
| `--space-20` | `80px` |
| `--space-24` | `96px` |

## Radius

| Token | Value |
| --- | --- |
| `--radius-none` | `0` |
| `--radius-xs` | `2px` |
| `--radius-sm` | `4px` |
| `--radius-md` | `6px` |
| `--radius-lg` | `8px` |
| `--radius-xl` | `10px` |
| `--radius-2xl` | `12px` |
| `--radius-3xl` | `16px` |
| `--radius-4xl` | `20px` |
| `--radius-full` | `9999px` |
| `--radius` | `var(--radius-2xl)` |

## Typography

| Token | Value |
| --- | --- |
| `--font-sans` | `-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Oxygen, Ubuntu, Cantarell, "Open Sans", "Helvetica Neue", sans-serif` |
| `--font-mono` | `ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace` |
| `--text-2xs` | `0.6875rem` |
| `--text-xs` | `0.75rem` |
| `--text-sm` | `0.8125rem` |
| `--text-base` | `0.875rem` |
| `--text-md` | `1rem` |
| `--text-lg` | `1.125rem` |
| `--text-xl` | `1.25rem` |
| `--text-2xl` | `1.5rem` |
| `--text-3xl` | `1.75rem` |
| `--text-4xl` | `2rem` |
| `--text-5xl` | `2.5rem` |
| `--text-6xl` | `3rem` |
| `--leading-none` | `1` |
| `--leading-tight` | `1.25` |
| `--leading-snug` | `1.375` |
| `--leading-normal` | `1.5` |
| `--leading-relaxed` | `1.625` |
| `--leading-loose` | `1.8` |
| `--weight-normal` | `400` |
| `--weight-medium` | `500` |
| `--weight-semibold` | `600` |
| `--weight-bold` | `700` |
| `--weight-extrabold` | `800` |
| `--tracking-tighter` | `-0.02em` |
| `--tracking-tight` | `-0.01em` |
| `--tracking-normal` | `0` |
| `--tracking-wide` | `0.02em` |
| `--tracking-wider` | `0.04em` |
| `--tracking-widest` | `0.08em` |
| `--measure-prose` | `65ch` |
| `--icon-xs` | `12px` |
| `--icon-sm` | `14px` |
| `--icon-md` | `16px` |
| `--icon-lg` | `20px` |
| `--icon-xl` | `24px` |

## Z-index layers

| Token | Value |
| --- | --- |
| `--z-below` | `-1` |
| `--z-base` | `0` |
| `--z-raised` | `10` |
| `--z-sticky` | `100` |
| `--z-dropdown` | `1000` |
| `--z-overlay` | `2000` |
| `--z-drawer` | `2500` |
| `--z-modal` | `3000` |
| `--z-popover` | `3500` |
| `--z-toast` | `4000` |
| `--z-tooltip` | `5000` |
| `--z-notification` | `6000` |
| `--z-max` | `9999` |

## Motion (durations + easing)

| Token | Value |
| --- | --- |
| `--motion-instant` | `0ms` |
| `--motion-fast` | `120ms` |
| `--motion-base` | `160ms` |
| `--motion-moderate` | `200ms` |
| `--motion-slow` | `300ms` |
| `--motion-slower` | `500ms` |
| `--motion-spinner` | `1s` |
| `--spin-duration` | `var(--motion-spinner)` |
| `--ease-standard` | `cubic-bezier(0.2, 0, 0, 1)` |
| `--ease-emphasized` | `cubic-bezier(0.3, 0, 0, 1)` |
| `--ease-in` | `cubic-bezier(0.4, 0, 1, 1)` |
| `--ease-out` | `cubic-bezier(0, 0, 0.2, 1)` |
| `--ease-in-out` | `cubic-bezier(0.4, 0, 0.2, 1)` |

## Elevation (composed shadows)

| Token | Value |
| --- | --- |
| `--shadow-xs` | `0 1px 2px var(--color-shadow-xs)` |
| `--shadow-sm` | `0 1px 3px var(--color-shadow-sm), 0 1px 2px var(--color-shadow-xs)` |
| `--shadow-md` | `0 4px 6px var(--color-shadow-sm), 0 2px 4px var(--color-shadow-xs)` |
| `--shadow-lg` | `0 10px 15px var(--color-shadow-md), 0 4px 6px var(--color-shadow-sm)` |
| `--shadow-xl` | `0 20px 25px var(--color-shadow-md), 0 8px 10px var(--color-shadow-sm)` |
| `--shadow-2xl` | `0 25px 50px var(--color-shadow-lg)` |

## Breakpoints (reference)

| Token | Value |
| --- | --- |
| `--bp-xs` | `480px` |
| `--bp-sm` | `640px` |
| `--bp-md` | `768px` |
| `--bp-lg` | `1024px` |
| `--bp-xl` | `1280px` |

## Density

| Token | Value |
| --- | --- |
| `--density-row-py` | `var(--space-3)` |
| `--density-row-px` | `var(--space-3-5)` |
| `--density-gap` | `var(--space-3)` |
| `--density-control-h` | `40px` |

## Layout shell

| Token | Value |
| --- | --- |
| `--sidebar-width` | `clamp(220px, 18vw, 280px)` |
| `--sidebar-width-min` | `200px` |
| `--sidebar-width-max` | `320px` |
| `--sidebar-width-collapsed` | `72px` |
| `--gutter` | `var(--space-6)` |
| `--grid-card-min` | `200px` |

## Color — text

| Token | Value |
| --- | --- |
| `--color-text` | `light-dark(#2d3748, #e2e8f0)` |
| `--color-text-heading` | `light-dark(#1e293b, #f1f5f9)` |
| `--color-text-secondary` | `light-dark(#475569, #cbd5e1)` |
| `--color-text-muted` | `light-dark(#5e6a78, #9fadbe)` |
| `--color-text-subtle` | `light-dark(#5e6a78, #9fadbe)` |
| `--color-text-faint` | `light-dark(#5e6a78, #9fadbe)` |
| `--color-text-dark` | `var(--color-text-secondary)` |
| `--color-text-dim` | `var(--color-text-secondary)` |
| `--color-text-black` | `var(--color-text)` |
| `--color-text-gray` | `var(--color-text-muted)` |
| `--color-text-medium` | `var(--color-text-muted)` |
| `--color-text-faint2` | `var(--color-text-faint)` |
| `--color-text-light` | `var(--color-text-faint)` |
| `--color-text-placeholder` | `var(--color-text-faint)` |
| `--color-text-navy` | `#1a1a2e` |

## Color — background

| Token | Value |
| --- | --- |
| `--color-bg-page` | `light-dark(#f5f7fa, #0f172a)` |
| `--color-bg-surface` | `light-dark(#ffffff, #1e293b)` |
| `--color-bg-input` | `light-dark(#f9fafb, #0f172a)` |
| `--color-bg-hover` | `light-dark(#f8fafc, #334155)` |
| `--color-bg-muted` | `light-dark(#f0f3f7, #1a2540)` |
| `--color-bg-subtle` | `light-dark(#f8f9fa, #162032)` |
| `--color-bg-alt` | `light-dark(#f7fafc, #0f172a)` |
| `--color-bg-input-alt` | `light-dark(#edf2f7, #253045)` |
| `--color-bg-empty` | `light-dark(#f0f0f0, #253045)` |
| `--color-bg-off-white` | `#fafbfd` |

## Color — border

| Token | Value |
| --- | --- |
| `--color-border` | `light-dark(#e2e8f0, #334155)` |
| `--color-border-light` | `light-dark(#f1f5f9, #334155)` |
| `--color-border-medium` | `light-dark(#cbd5e0, #475569)` |
| `--color-border-faint` | `light-dark(#e0e6ed, #2a3650)` |
| `--color-border-subtle` | `light-dark(#e0e5e8, #2a3650)` |
| `--color-border-xfaint` | `light-dark(#f0f0f0, #1e293b)` |
| `--color-border-ddd` | `light-dark(#ddd, #334155)` |
| `--color-border-dark-faint` | `rgba(255, 255, 255, 0.03)` |

## Color — accent / brand

| Token | Value |
| --- | --- |
| `--color-accent` | `#ff5e3a` |
| `--color-accent-hover` | `light-dark(#e04520, #ff7a5c)` |
| `--color-accent-text` | `light-dark(#cc3a16, #ff8a5c)` |
| `--color-on-accent` | `#ffffff` |
| `--color-focus-ring` | `#ff5e3a` |
| `--color-logo-gradient` | `linear-gradient(135deg, #ff5e3a 0%, #ff8a5c 100%)` |
| `--color-accent-gradient` | `linear-gradient(135deg, #ff5e3a 0%, #ff2d55 100%)` |
| `--color-accent-shadow` | `rgba(255, 94, 58, 0.3)` |
| `--color-accent-ring` | `light-dark(rgba(255, 94, 58, 0.1), rgba(255, 94, 58, 0.15))` |
| `--color-accent-tint` | `light-dark(#fff5f3, #2a1a15)` |
| `--color-accent-mid` | `#ff8a5c` |
| `--color-accent-shadow-lg` | `rgba(255, 94, 58, 0.4)` |
| `--color-accent-bg` | `rgba(255, 94, 58, 0.06)` |
| `--color-accent-bg-sm` | `rgba(255, 94, 58, 0.08)` |
| `--color-accent-ring-dark` | `rgba(255, 94, 58, 0.15)` |
| `--color-accent-ring-strong` | `rgba(255, 94, 58, 0.2)` |
| `--color-accent-ring-xl` | `rgba(255, 94, 58, 0.4)` |
| `--color-accent-ring-xs` | `rgba(255, 94, 58, 0.05)` |
| `--color-primary` | `var(--color-accent)` |
| `--color-primary-hover` | `var(--color-accent-hover)` |
| `--color-accent-second` | `#ff2d55` |
| `--color-accent-shadow-sm` | `rgba(255, 94, 58, 0.25)` |

## Color — semantic (success/warn/danger/info)

| Token | Value |
| --- | --- |
| `--color-error-bg` | `light-dark(#fee2e2, #3b1111)` |
| `--color-error-text` | `light-dark(#b91c1c, #fca5a5)` |
| `--color-success-bg` | `light-dark(#dcfce7, #052e16)` |
| `--color-success-text` | `light-dark(#15803d, #86efac)` |
| `--color-success-border` | `#16a34a` |
| `--color-success-alt` | `#16a34a` |
| `--color-success-bg-alt` | `var(--color-success-bg)` |
| `--color-success-text-alt` | `var(--color-success-text)` |
| `--color-success-bg-green` | `var(--color-success-bg)` |
| `--color-success-text-green` | `var(--color-success-text)` |
| `--color-danger-bg` | `#ef4444` |
| `--color-danger-text` | `#ffffff` |
| `--color-danger-bg-hover` | `#dc2626` |
| `--color-danger-alt` | `#ef4444` |
| `--color-danger-ring` | `rgba(239, 68, 68, 0.3)` |
| `--color-danger-ring-lg` | `rgba(239, 68, 68, 0.4)` |
| `--color-danger-light-bg` | `light-dark(#fef2f2, #2a0c0c)` |
| `--color-danger-lighter` | `light-dark(#fef2f2, #2a0c0c)` |
| `--color-danger-text-alt` | `var(--color-error-text)` |
| `--color-danger-gradient` | `linear-gradient(135deg, #ef4444 0%, #dc2626 100%)` |
| `--color-warning-bg` | `light-dark(#fef3c7, #2a2410)` |
| `--color-warning-text` | `light-dark(#b45309, #fbbf24)` |
| `--color-warning-border` | `#f59e0b` |
| `--color-warning-bg-dark` | `light-dark(#fde68a, #3d2e00)` |
| `--color-warning-ring` | `rgba(245, 158, 11, 0.12)` |
| `--color-warning-shadow` | `rgba(245, 158, 11, 0.4)` |
| `--color-warning-orange-bg` | `var(--color-warning-bg)` |
| `--color-warning-orange-border` | `var(--color-warning-border)` |
| `--color-warning-orange-text` | `var(--color-warning-text)` |
| `--color-warning-bg-light` | `var(--color-warning-bg)` |
| `--color-warning-text-amber` | `var(--color-warning-text)` |
| `--color-warning-bg-orange` | `var(--color-warning-bg)` |
| `--color-warning-text-orange` | `var(--color-warning-text)` |
| `--color-info-bg` | `light-dark(#eff6ff, #0c2d48)` |
| `--color-info-text` | `light-dark(#1d4ed8, #93c5fd)` |
| `--color-info-border` | `#3b82f6` |
| `--color-info-blue` | `#3b82f6` |
| `--color-info-bg-alt` | `var(--color-info-bg)` |
| `--color-info-text-alt` | `var(--color-info-text)` |
| `--color-info-surface` | `var(--color-info-bg)` |
| `--color-danger-hover-bg` | `rgba(239, 68, 68, 0.1)` |
| `--color-success-ring` | `rgba(72, 187, 120, 0.1)` |
| `--color-success-ring-dark` | `rgba(72, 187, 120, 0.15)` |
| `--color-success-text-strong` | `#2f855a` |
| `--color-success-ring-vivid` | `rgba(74, 222, 128, 0.1)` |
| `--color-success-text-vivid` | `#86efac` |
| `--color-success-ring-vivid-lg` | `rgba(74, 222, 128, 0.15)` |
| `--color-success-icon-vivid` | `#4ade80` |
| `--color-info-border-light` | `#90cdf4` |
| `--color-warning-ring-xs` | `rgba(255, 193, 7, 0.05)` |
| `--color-warning-bg-faint` | `#fffbeb` |
| `--color-error-text-dark` | `light-dark(#991b1b, #f87171)` |
| `--color-danger-shadow` | `rgba(220, 38, 38, 0.2)` |
| `--color-danger-shadow-lg` | `rgba(220, 38, 38, 0.3)` |

## Color — shadow alphas / overlays

| Token | Value |
| --- | --- |
| `--color-shadow` | `light-dark(rgba(0, 0, 0, 0.1), rgba(0, 0, 0, 0.3))` |
| `--color-shadow-lg` | `light-dark(rgba(0, 0, 0, 0.12), rgba(0, 0, 0, 0.38))` |
| `--color-shadow-xs` | `rgba(0, 0, 0, 0.05)` |
| `--color-shadow-sm` | `rgba(0, 0, 0, 0.08)` |
| `--color-shadow-md` | `light-dark(rgba(0, 0, 0, 0.15), rgba(0, 0, 0, 0.34))` |
| `--color-shadow-xl` | `rgba(0, 0, 0, 0.2)` |
| `--color-shadow-2xl` | `rgba(0, 0, 0, 0.25)` |
| `--color-shadow-3xl` | `rgba(0, 0, 0, 0.3)` |
| `--color-shadow-4xl` | `rgba(0, 0, 0, 0.4)` |
| `--color-overlay` | `rgba(0, 0, 0, 0.5)` |
| `--color-overlay-light` | `rgba(0, 0, 0, 0.45)` |
| `--color-overlay-heavy` | `rgba(0, 0, 0, 0.85)` |
| `--color-overlay-darkest` | `rgba(0, 0, 0, 0.92)` |
| `--color-overlay-shadow` | `rgba(0, 0, 0, 0.6)` |
| `--color-on-overlay` | `rgba(255, 255, 255, 0.95)` |
| `--color-on-overlay-muted` | `rgba(255, 255, 255, 0.9)` |
| `--color-overlay-button` | `rgba(255, 255, 255, 0.12)` |
| `--color-overlay-button-hover` | `rgba(255, 255, 255, 0.22)` |
| `--color-overlay-mid` | `rgba(0, 0, 0, 0.6)` |
| `--color-overlay-video` | `rgba(0, 0, 0, 0.55)` |

## Color — sidebar

| Token | Value |
| --- | --- |
| `--color-sidebar-bg-from` | `light-dark(#2a3042, #0f172a)` |
| `--color-sidebar-bg-to` | `light-dark(#232838, #0c1322)` |
| `--color-sidebar-text` | `rgba(255, 255, 255, 0.65)` |
| `--color-sidebar-text-hover` | `rgba(255, 255, 255, 0.9)` |
| `--color-sidebar-text-active` | `#ffffff` |
| `--color-sidebar-active-bg` | `rgba(255, 94, 58, 0.12)` |
| `--color-sidebar-hover-bg` | `rgba(255, 255, 255, 0.06)` |
| `--color-sidebar-separator` | `rgba(255, 255, 255, 0.07)` |
| `--color-sidebar-overlay` | `rgba(0, 0, 0, 0.5)` |
| `--color-sidebar-storage-bg` | `rgba(255, 255, 255, 0.05)` |
| `--color-sidebar-storage-border` | `rgba(255, 255, 255, 0.07)` |
| `--color-sidebar-storage-text` | `rgba(255, 255, 255, 0.8)` |
| `--color-sidebar-storage-bar` | `rgba(255, 255, 255, 0.1)` |
| `--color-sidebar-storage-faint` | `rgba(255, 255, 255, 0.5)` |
| `--color-sidebar-logo-gradient` | `linear-gradient(135deg, #ff5e3a 0%, #ff8a5c 100%)` |
| `--color-sidebar-progress` | `linear-gradient(90deg, #ff5e3a 0%, #ff8a5c 100%)` |
| `--color-sidebar-shadow` | `rgba(255, 94, 58, 0.35)` |
| `--color-sidebar-shadow-lg` | `rgba(255, 94, 58, 0.45)` |

## Color — file types

| Token | Value |
| --- | --- |
| `--color-ft-html` | `#e34c26` |
| `--color-ft-js` | `#2965f1` |
| `--color-ft-python` | `#3776ab` |
| `--color-ft-typescript` | `#3178c6` |
| `--color-ft-rust` | `#dea584` |
| `--color-ft-go` | `#00add8` |
| `--color-ft-java` | `#e76f00` |
| `--color-ft-shell` | `#555555` |
| `--color-ft-csharp` | `#68217a` |
| `--color-ft-php` | `#8892be` |
| `--color-ft-ruby` | `#cc342d` |
| `--color-ft-swift` | `#fa7343` |
| `--color-ft-kotlin` | `#7f52ff` |
| `--color-ft-scala` | `#e38c00` |
| `--color-ft-angular` | `#cb171e` |
| `--color-ft-cpp` | `#9c4221` |
| `--color-ft-docker` | `#083fa1` |
| `--color-ft-generic-blue` | `#556ee6` |
| `--color-ft-generic-green` | `#4eaa25` |
| `--color-ft-generic-gray` | `#a0aec0` |
| `--color-ft-orange-light` | `#ffb86c` |
| `--color-ft-yellow` | `#ffd43b` |
| `--color-ft-orange-alt` | `#e34c26` |
| `--color-ft-coffeescript` | `#9c4221` |
| `--color-ft-folder-bg` | `#ffeaa7` |
| `--color-ft-folder-tab` | `#fdcb6e` |
| `--color-ft-doc-bg` | `#e0ecff` |
| `--color-ft-doc-text` | `#3171d8` |
| `--color-ft-pdf-bg` | `#fee2e2` |
| `--color-ft-pdf-text` | `#e53e3e` |
| `--color-ft-image-bg` | `#e0f2fe` |
| `--color-ft-image-text` | `#3b82f6` |
| `--color-ft-video-bg-from` | `#ede9fe` |
| `--color-ft-video-bg-to` | `#fce7f3` |
| `--color-ft-video-text` | `#8b5cf6` |
| `--color-ft-audio-bg` | `#fef3c7` |
| `--color-ft-audio-text` | `#f59e0b` |
| `--color-ft-audio-alt-bg` | `#fff3e0` |
| `--color-ft-spreadsheet-bg` | `#e6f4ea` |
| `--color-ft-spreadsheet-text` | `#0d904f` |
| `--color-ft-presentation-bg` | `#fef3e2` |
| `--color-ft-presentation-text` | `#d04423` |
| `--color-ft-archive-bg` | `#f5f0eb` |
| `--color-ft-archive-text` | `#8d6e63` |
| `--color-ft-installer-bg` | `#f3e8ff` |
| `--color-ft-installer-text` | `#7c3aed` |
| `--color-ft-script-bg` | `#e8f5e9` |
| `--color-ft-script-text` | `#4eaa25` |
| `--color-ft-config-bg` | `#f1f3f5` |
| `--color-ft-config-text` | `#718096` |

## Color — calendar dots

| Token | Value |
| --- | --- |
| `--color-cal-1` | `#d36868` |
| `--color-cal-2` | `#d3a268` |
| `--color-cal-3` | `#c9d368` |
| `--color-cal-4` | `#8fd368` |
| `--color-cal-5` | `#68d37c` |
| `--color-cal-6` | `#68d3b6` |
| `--color-cal-7` | `#68b6d3` |
| `--color-cal-8` | `#687cd3` |
| `--color-cal-9` | `#8f68d3` |
| `--color-cal-10` | `#c968d3` |
| `--color-cal-11` | `#d368a2` |

## Color — badges

| Token | Value |
| --- | --- |
| `--color-badge-success-bg` | `#ecfdf5` |
| `--color-badge-success-bg-medium` | `#d1fae5` |
| `--color-badge-success-text` | `#065f46` |
| `--color-badge-success-border` | `#a7f3d0` |
| `--color-badge-success-fill` | `#047857` |
| `--color-badge-success-fill-dark` | `#064e27` |
| `--color-badge-success-fill-faint` | `#f0fdf4` |
| `--color-badge-green-bg` | `#ecfdf5` |
| `--color-badge-green-text` | `#065f46` |
| `--color-badge-orange-bg` | `light-dark(#fff5f3, #2a1814)` |
| `--color-badge-orange-text` | `light-dark(#ff5e3a, #ff8a65)` |
| `--color-badge-error-border` | `#fecaca` |
| `--color-badge-warning-text` | `#92400e` |
| `--color-badge-warning-border` | `#fde68a` |
| `--color-badge-amber-bg` | `#fef3c7` |
| `--color-badge-amber-text` | `#f59e0b` |
| `--color-badge-indigo-bg` | `#ede9fe` |
| `--color-badge-indigo-text` | `#6d28d9` |
| `--color-badge-blue-bg` | `light-dark(#eff6ff, #0c2d48)` |
| `--color-badge-blue-text` | `light-dark(#1e40af, #93c5fd)` |
| `--color-badge-blue-border` | `#bfdbfe` |
| `--color-badge-gray` | `#d1d5db` |

## Color — other

| Token | Value |
| --- | --- |
| `--color-item` | `var(--color-bg-surface)` |
| `--color-item-hover` | `var(--color-bg-hover)` |
| `--color-item-active` | `light-dark(#f8d2ae, #5a5047)` |
| `--color-item-selected` | `light-dark(#fff8f6, #39281a)` |
| `--color-item-hover-accent` | `light-dark(#fff0ec, #3d342c)` |
| `--color-item-hover-blue` | `#f0f8ff` |
| `--color-item-hover-sky` | `#e0f2fe` |
| `--color-multiselect-bg` | `#1e293b` |
| `--color-multiselect-border` | `#334155` |
| `--color-multiselect-text` | `#ffffff` |
| `--color-multiselect-text-faint` | `rgba(255, 255, 255, 0.7)` |
| `--color-multiselect-hover-bg` | `rgba(255, 255, 255, 0.1)` |
| `--color-multiselect-action-text` | `#ffffff` |
| `--color-multiselect-action-hover` | `rgba(255, 255, 255, 0.2)` |
| `--color-multiselect-danger-bg` | `rgba(239, 68, 68, 0.25)` |
| `--color-multiselect-danger-text` | `#fca5a5` |
| `--color-multiselect-danger-active` | `rgba(239, 68, 68, 0.4)` |
| `--color-multiselect-danger-text-active` | `#ffffff` |
| `--color-notification-bg` | `light-dark(#ffffff, #1e293b)` |
| `--color-notification-badge` | `#ff3b30` |
| `--color-notification-success` | `#34c759` |
| `--color-notification-error` | `#ff3b30` |
| `--color-lightbox-overlay` | `rgba(0, 0, 0, 0.92)` |
| `--color-lightbox-btn-bg` | `rgba(255, 255, 255, 0.12)` |
| `--color-lightbox-btn-text` | `#ffffff` |
| `--color-lightbox-btn-hover` | `rgba(255, 255, 255, 0.25)` |
| `--color-lightbox-gradient-top` | `linear-gradient(to bottom, rgba(0, 0, 0, 0.6), transparent)` |
| `--color-lightbox-gradient-bottom` | `linear-gradient(to top, rgba(0, 0, 0, 0.6), transparent)` |
| `--color-lightbox-text-faint` | `rgba(255, 255, 255, 0.5)` |
| `--color-lightbox-text-muted` | `rgba(255, 255, 255, 0.7)` |
| `--color-oidc-bg` | `var(--color-info-blue)` |
| `--color-oidc-shadow` | `rgba(79, 70, 229, 0.3)` |
| `--color-oidc-shadow-lg` | `rgba(79, 70, 229, 0.4)` |
| `--color-device-verify-text` | `#ffc107` |
| `--color-device-verify-shadow` | `rgba(255, 193, 7, 0.5)` |
| `--color-device-verify-drop-shadow` | `rgba(255, 193, 7, 0.4)` |
| `--color-device-verify-border` | `#ffc107` |
| `--color-device-verify-muted` | `#6c757d` |
| `--color-device-verify-dim` | `#ccc` |
| `--color-content-muted` | `#888` |
| `--color-content-bg-warn` | `light-dark(#ffeaa7, #3d2e00)` |
| `--color-content-bg-warn-dark` | `light-dark(#fdcb6e, #5a4200)` |
| `--color-user-menu-header-bg` | `light-dark(linear-gradient(135deg, #fef5f3 0%, #fdf2f8 100%), linear-gradient(135deg, #1a2332 0%, #1e2940 100%))` |
| `--color-user-menu-header-border` | `light-dark(#fce7e1, #3a2520)` |
| `--color-share-link-text` | `var(--color-info-text)` |
| `--color-share-link-hover` | `var(--color-info-text)` |
| `--color-share-remove-text` | `#b71c1c` |
| `--color-share-owner-text` | `#757575` |
| `--color-recent-muted` | `#6c757d` |
| `--color-recent-border` | `#6c757d` |
| `--color-star-text` | `#fbbf24` |
| `--color-star-text-hover` | `#f59e0b` |
| `--color-star-active` | `#d97706` |
| `--color-card-drop-tint` | `rgba(230, 126, 34, 0.08)` |
| `--color-card-drop-border` | `#e67e22` |
| `--color-neutral-warm-bg` | `#f5f0eb` |
| `--color-neutral-warm-text` | `#8d6e63` |
| `--color-neutral-bg` | `#f1f3f5` |
| `--color-admin-blue` | `#60a5fa` |
| `--color-admin-blue-bg` | `rgba(59, 130, 246, 0.1)` |
| `--color-admin-blue-bg-sm` | `rgba(59, 130, 246, 0.15)` |
| `--color-secret-green` | `#059669` |
| `--color-progress-overlay` | `rgba(255, 255, 255, 0.95)` |
| `--color-progress-overlay-dark` | `rgba(30, 41, 59, 0.95)` |
| `--color-black` | `#000000` |
| `--color-notification-error-ring` | `rgba(255, 59, 48, 0.1)` |
| `--color-avatar-gradient` | `linear-gradient(135deg, #3b82f6, #6366f1)` |
| `--color-role-admin-bg` | `#dbeafe` |
| `--color-role-admin-text` | `#1d4ed8` |
| `--color-dark-mid` | `#475569` |
| `--color-role-admin-dark-bg` | `#1e3a5f` |
| `--color-photo-check-border` | `rgba(255, 255, 255, 0.8)` |
| `--color-storage-fill-green` | `linear-gradient(90deg, #059669, #10b981)` |
| `--color-storage-fill-orange` | `linear-gradient(90deg, #d97706, #f59e0b)` |
| `--color-storage-fill-red` | `linear-gradient(90deg, #dc2626, #ef4444)` |
| `--color-stat-warn-border` | `#fbbf24` |
| `--color-dark-footer` | `#162032` |
| `--color-scrollbar-dark` | `rgba(255, 255, 255, 0.15)` |
| `--color-music-gradient` | `linear-gradient(135deg, #667eea 0%, #764ba2 100%)` |
| `--color-music-background` | `var(--color-bg-surface)` |
| `--color-music-public-bg` | `rgba(74, 144, 217, 0.12)` |
| `--color-video-play` | `#ffffff` |
| `--color-video-play-shadow` | `#000000` |
