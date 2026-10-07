# MieoSQL — Design System (Apple-native developer tool)

Hướng: **macOS native + Xcode + Finder**, dành cho database/Kafka developer.
Không glassmorphism, không gradient, không floating cards, không neon.

## Tokens (đã map trong `frontend/src/styles.css`)

| Token (Tailwind) | Giá trị | Dùng cho |
|---|---|---|
| `bg-window` | `#F5F5F7` | nền cửa sổ, toolbar |
| `bg-sidebar` | `#F2F2F7` | sidebar, inspector, status bar |
| `bg-surface` | `#FFFFFF` | content, editor, grid |
| `bg-accent` / `text-accent` | `#007AFF` | action chính, selection, focus, link |
| `bg-accent-hover` | `#0066D6` | hover của action chính |
| `border-hairline` | `#D2D2D7` | border 1px duy nhất được dùng |
| `text-slate-900` | `#1D1D1F` | text chính |
| `text-slate-600` | `#6E6E73` | text phụ |
| `text-slate-500` | `#8E8E93` | text mờ, icon mặc định |
| `text-slate-300` | `#D2D2D7` | disabled / hairline |
| success `text-emerald-600` | `#28A745` (marker `#34C759`) | OK, connected |
| warning `text-amber-600` | `#E68200` (marker `#FF9500`) | transaction, cảnh báo |
| error `text-rose-600` | `#E5352B` (marker `#FF3B30`) | lỗi, destructive |

Các hue `violet / fuchsia / indigo / cyan / teal / purple` đã bị **gộp về thang slate** (monochrome).
`orange` = `amber`. Không dùng màu khác ngoài bảng trên.

### Radii
`rounded` = 7px (button/input) · `rounded-sm` 6 · `rounded-md` 8 · `rounded-lg` 10 · `rounded-xl` 12 (modal/window).
Không pill (trừ scrollbar/avatar/badge đếm). Không bo > 12px.

### Typography
UI: SF Pro Text / Inter (`font-sans`) · Code/SQL/JSON: SF Mono / JetBrains Mono (`font-code`).

| Vai trò | Size / weight |
|---|---|
| Window title, toolbar label | 13px / 500 |
| Sidebar, body, button, menu | 13px / 400–500 |
| Page title | 20px / 600 |
| Section title | 15px / 600 |
| Section header (SIDEBAR CAPS) | 11px / 600 / uppercase / tracking rộng |
| Table cell | 12.5px / 400 |
| Table head | 11.5px / 600 |
| SQL / JSON / log | 12.5px mono |
| Status bar | 11px / 400 |

### Khoảng cách & kích thước chrome
titlebar 28px · menubar 24px · toolbar 48px · tab bar 34px · query toolbar 40px · status bar 24px.
Sidebar 240px · inspector 280px. Spacing 4 / 8 / 12 / 16 / 24 / 32.

## Nguyên tắc bắt buộc

1. **Chỉ 1 màu accent** (`accent` = Apple Blue). Icon mặc định `text-slate-500`; active/hover mới dùng accent.
2. **Không gradient** (trừ không có gì).
3. **Icon một màu** — 16–18px chrome, 12–14px trong panel, `strokeWidth` 1.6–1.8. Màu semantic (`emerald/amber/rose`) chỉ dành cho *status dot / badge trạng thái*, không cho icon trang trí.
4. **Ưu tiên tương phản nền thay cho border** — chỉ dùng border 1px `hairline` hoặc `slate-200` khi thật cần (table lines, divider panel).
5. **Không để mọi action cùng nổi bật** — primary action duy nhất dùng `bg-accent`; còn lại là text/icon button `hover:bg-black/[0.05]`.
6. **Hover style chuẩn**: `hover:bg-slate-900/[0.05]`; selected: `bg-accent/10` hoặc `bg-accent/15` (không đổ accent đặc, trừ primary button).
7. **Shadow tối thiểu**: menu/dropdown/modal `shadow-lg`/`shadow-xl`; panel và toolbar không shadow.
8. **SQL/JSON/Kafka message luôn `font-code`**.
9. **Không thêm border trên sidebar/inspector** — phân tách bằng nền `bg-sidebar` vs `bg-surface`.
10. **Focus**: `focus:outline-none` + ring accent (`focus:ring-2 focus:ring-accent/40`), input radius `rounded-md`.

## Component recipes

- **Toolbar button**: `flex h-7 items-center gap-1.5 rounded-md px-2 text-[13px] font-medium text-slate-700 hover:bg-slate-900/[0.05]`; icon 16px `text-slate-500`; primary CTA `bg-accent text-white hover:bg-accent-hover`.
- **Icon button**: `grid h-7 w-7 place-items-center rounded-md text-slate-500 hover:bg-slate-900/[0.05] hover:text-slate-700`.
- **Sidebar row**: `h-6 rounded-md px-2 text-[13px] text-slate-700`; hover `bg-slate-900/[0.04]`; selected `bg-accent/[0.12] text-slate-900`.
- **Section header**: `px-3 py-1.5 text-[11px] font-semibold uppercase tracking-[0.06em] text-slate-500`.
- **Inspector tabs**: `h-8 text-[12.5px]`; active `text-slate-900` + `border-b-2 border-accent`; inactive `text-slate-500`.
- **Primary button**: `h-7 rounded-md bg-accent px-2.5 text-[13px] font-medium text-white hover:bg-accent-hover`.
- **Secondary button**: `h-7 rounded-md border border-slate-300 bg-white px-2.5 text-[13px] text-slate-700 hover:bg-slate-50`.
- **Badge**: `rounded px-1 text-[10.5px] font-semibold` — neutral `bg-slate-900/[0.06] text-slate-600`; semantic chỉ khi là trạng thái.
