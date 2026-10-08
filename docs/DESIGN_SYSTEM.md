# MieoSQL — Design System (Apple-native developer tool)

Hướng: **macOS native + Xcode + Finder**, dành cho database/Kafka developer.
Không glassmorphism, không gradient, không floating cards, không neon.

## Tokens (đã map trong `frontend/src/styles.css`)

| Token (Tailwind) | Giá trị | Dùng cho |
|---|---|---|
| `bg-window` | `#F5F5F7` | nền cửa sổ, toolbar |
| `bg-sidebar` | `#F2F2F7` | sidebar, status bar |
| `bg-inspector` | `#F7F7F9` | inspector phải |
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
UI / Data grid: Inter 13px (`font-sans`) · SQL editor: JetBrains Mono 13px / 1.25 · Numbers: JetBrains Mono 12px (`font-num`) · Terminal / logs / messages: JetBrains Mono 12px (`font-log`, `font-code`).

| Vai trò | Font / Size |
|---|---|
| UI (window, sidebar, body, button, menu) | Inter 13px |
| SQL editor | JetBrains Mono 13px / 1.25 |
| Data grid | Inter 13px |
| Numbers | JetBrains Mono 12px |
| Terminal / logs / messages | JetBrains Mono 12px |

| Vai trò | Size / weight |
|---|---|
| Window title, toolbar label | 13px / 500 |
| Sidebar, body, button, menu | 13px / 400–500 |
| Page title | 20px / 600 |
| Section title | 15px / 600 |
| Section header (SIDEBAR CAPS) | 11px / 600 / uppercase / tracking rộng |
| Table cell (grid) | 13px Inter / 400 |
| Table head | 11.5px / 600 |
| Numbers | 12px JetBrains Mono |
| Terminal / logs / messages | 12px JetBrains Mono |
| Status bar | 11px / 400 |

### Khoảng cách & kích thước chrome
titlebar 28px · menubar 24px · toolbar 48px · tab bar 34px · query toolbar 40px · status bar 24px.
Sidebar 240px · inspector 280px. Spacing 4 / 8 / 12 / 16 / 24 / 32.

## Nguyên tắc bắt buộc

1. **Chỉ 1 màu accent** (`accent` = Apple Blue). Icon mặc định `text-slate-600` (#6E6E73); hover `text-slate-900` (#1D1D1F); active/selected `text-accent` (#007AFF); disabled `text-slate-300` (#C7C7CC → #D2D2D7). Icon trang trí không bao giờ mang màu khác.
2. **Không gradient.**
3. **Icon một hệ outline (lucide)** — toolbar 16–18px stroke **1.5**; panel/tree 11–14px stroke 1.6–1.8. Cùng visual weight, không trộn filled/outline/emoji.
4. **Màu có ý nghĩa, không trang trí** — gray = bình thường; accent = selected/active/primary; emerald = connected/OK; amber = warning/key; rose = error/destructive.
5. **Bốn lớp nền tạo depth** — Window `#F5F5F7` · Sidebar `#F2F2F7` · Inspector `#F7F7F9` · Surface/Editor `#FFFFFF`.
6. **Separation**: giữ border 1px `hairline` giữa Sidebar | Content | Inspector; các chỗ còn lại ưu tiên tương phản nền thay vì border.
7. **Không để mọi action cùng nổi bật** — primary action duy nhất dùng `bg-accent`; còn lại là text/icon button `hover:bg-slate-900/[0.05]`.
8. **Shadow tối thiểu**: menu/dropdown/modal `shadow-lg`/`shadow-xl`; panel và toolbar không shadow.
9. **SQL editor / numbers / logs / messages dùng `font-code` (JetBrains Mono)** — editor 13px/1.25, numbers + logs 12px (`font-num`, `font-log`). Grid body và UI dùng Inter 13px.
10. **Focus**: `focus:outline-none` + ring accent (`focus:ring-2 focus:ring-accent/40`), input radius `rounded-md`.

## Component recipes

- **Toolbar button**: `flex h-7 items-center gap-1.5 rounded-md px-2 text-[13px] font-medium text-slate-700 hover:bg-slate-900/[0.05]`; icon 16px stroke 1.5 `text-slate-600 group-hover:text-slate-900`; primary CTA `bg-accent text-white hover:bg-accent-hover`.
- **Icon button**: `grid h-7 w-7 place-items-center rounded-md text-slate-500 hover:bg-slate-900/[0.05] hover:text-slate-700`.
- **Sidebar row**: `min-h-6 rounded-md px-2 text-[13px] text-slate-700`; icon `text-slate-500`; hover `bg-slate-900/[0.04]`; selected `bg-accent/[0.12] text-accent` (icon + label xanh).
- **Sidebar sections**: `FAVORITES` · `CONNECTIONS` · `MESSAGING` (chỉ hiện khi có kết nối thuộc nhóm), header `text-[11px] font-semibold uppercase tracking-[0.06em] text-slate-500`.
- **Session row**: 2 dòng — tên 13px/500 + engine 11px `text-slate-500`; badge version neutral.
- **Inspector tabs**: `h-8 text-[12.5px]`, icon 12px, active `text-slate-900` + `border-b-2 border-accent`; inactive `text-slate-500`.
- **Primary button**: `h-7 rounded-md bg-accent px-2.5 text-[13px] font-medium text-white hover:bg-accent-hover`.
- **Secondary button**: `h-7 rounded-md border border-slate-300 bg-white px-2.5 text-[13px] text-slate-700 hover:bg-slate-50`.
- **Badge**: `rounded px-1 text-[10.5px] font-semibold` — neutral `bg-slate-900/[0.06] text-slate-600`; semantic chỉ khi là trạng thái.
