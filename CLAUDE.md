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
**Other:** direction, empty, field, input-group, item, kbd, native-select, spinner, typography

Full component reference with descriptions:
- accordion: vertically stacked interactive headings revealing content
- alert / alert-dialog: callout for attention / modal expecting response
- avatar: image with fallback for representing users
- badge: small status indicator
- breadcrumb: path hierarchy links
- button / button-group: clickable actions / grouped buttons
- calendar / date-picker: date selection components
- card: header + content + footer container
- carousel: swipeable content (Embla)
- chart: Recharts-based visualizations
- checkbox / radio-group / switch / toggle / toggle-group: selection controls
- collapsible: expand/collapse panel
- combobox: autocomplete input with suggestions
- command: search and quick actions menu
- context-menu / dropdown-menu / menubar: action menus (right-click / button / persistent)
- data-table: TanStack Table powered datagrids
- dialog / drawer / sheet: overlay content panels
- empty: empty state placeholder
- field: accessible form field composition (label + control + help text)
- hover-card / popover / tooltip: contextual information overlays
- input / input-group / input-otp / textarea: text entry components
- item: content with media, title, description, actions
- kbd: keyboard shortcut display
- label: accessible form labels
- navigation-menu: website navigation links
- pagination: page navigation controls
- progress / skeleton / spinner: loading indicators
- resizable: draggable panel layouts
- scroll-area: custom cross-browser scrollbars
- select / native-select: option pickers
- separator: visual content divider
- sidebar: composable themeable sidebar
- slider: range value input
- sonner / toast: temporary notification messages
- table: responsive data tables
- tabs: layered content panels
- typography: heading/paragraph/list styles

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
