# MateHub -- Project Instructions

## Frontend Development

When working on the frontend (`frontend/` directory):

- **Framework:** Next.js 16 with App Router, TypeScript, Tailwind CSS v4
- **UI Library:** shadcn/ui -- do NOT write custom UI primitives, use shadcn components
- **Component installation:** `cd frontend && npx shadcn@latest add <component-name>`

### Available shadcn/ui Components

Before building UI, check if a suitable component exists. Install with `npx shadcn@latest add <name>`:

**Layout & Structure:** accordion, aspect-ratio, card, carousel, collapsible, resizable, scroll-area, separator, sidebar, tabs
**Forms & Input:** button, button-group, calendar, checkbox, combobox, date-picker, field, input, input-group, input-otp, label, native-select, radio-group, select, slider, switch, textarea, toggle, toggle-group
**Overlay & Feedback:** alert, alert-dialog, dialog, drawer, dropdown-menu, context-menu, hover-card, popover, sheet, sonner (toast), toast, tooltip
**Data Display:** avatar, badge, breadcrumb, chart, data-table, empty, item, kbd, pagination, progress, skeleton, spinner, table, typography
**Navigation:** command, menubar, navigation-menu, direction

Full docs: https://ui.shadcn.com/docs/components
Community components: https://ui.shadcn.com/docs/directory

### Frontend Conventions

- Components go in `frontend/components/`
- UI primitives (shadcn) in `frontend/components/ui/`
- Feature components in `frontend/components/<feature>/` (e.g., `hub/`, `chat/`)
- Hooks in `frontend/hooks/`
- Utility functions in `frontend/lib/`
- Use `cn()` from `@/lib/utils` for conditional classNames
- SDK imports: `@matehub/sdk-video`, `@matehub/sdk-chat`

## Rust Backend

- Cargo workspace at repo root
- Services in `services/<name>/`
- Shared dependencies in root `Cargo.toml` `[workspace.dependencies]`
- Video service uses str0m (OpenSSL dependency -- build with debian, not alpine)

## Terminology

- **Hub** -- top-level tenant entity (like Discord Server)
- **Channel** -- voice/text/stage within a Hub
- **SFU Session** -- transient media session (Video Service)
