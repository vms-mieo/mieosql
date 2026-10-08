import { X } from "lucide-react";

import type {
  DensitySetting,
  FontSizeSetting,
  ThemeSetting,
} from "../lib/store";
import { useStore } from "../lib/store";

const THEMES: { value: ThemeSetting; label: string; hint: string }[] = [
  { value: "system", label: "System", hint: "Follow the OS appearance" },
  { value: "light", label: "Light", hint: "Always use the light theme" },
  { value: "dark", label: "Dark", hint: "Always use the dark theme" },
];

const FONT_SIZES: { value: FontSizeSetting; label: string; hint: string }[] = [
  { value: "small", label: "Small", hint: "12px" },
  { value: "medium", label: "Medium", hint: "13px" },
  { value: "large", label: "Large", hint: "14px" },
];

const DENSITIES: { value: DensitySetting; label: string; hint: string }[] = [
  { value: "comfortable", label: "Comfortable", hint: "30px rows" },
  { value: "compact", label: "Compact", hint: "26px rows" },
];

function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section>
      <h3 className="mb-2 text-[11px] font-semibold tracking-wide text-slate-500 uppercase">
        {title}
      </h3>
      <div className="space-y-2">{children}</div>
    </section>
  );
}

function RadioRow<T extends string>({
  name,
  value,
  checked,
  label,
  hint,
  onPick,
}: {
  name: string;
  value: T;
  checked: boolean;
  label: string;
  hint: string;
  onPick: (value: T) => void;
}) {
  return (
    <label className="flex cursor-pointer items-center gap-2.5 rounded-md px-1 py-0.5 hover:bg-slate-900/[0.04]">
      <input
        type="radio"
        name={name}
        checked={checked}
        onChange={() => onPick(value)}
        className="h-3.5 w-3.5 accent-[#007aff]"
      />
      <span className="text-[13px] text-slate-800">{label}</span>
      <span className="text-[11px] text-slate-400">{hint}</span>
    </label>
  );
}

function ToggleRow({
  label,
  hint,
  checked,
  onToggle,
}: {
  label: string;
  hint: string;
  checked: boolean;
  onToggle: (value: boolean) => void;
}) {
  return (
    <label className="flex cursor-pointer items-center justify-between gap-3 rounded-md px-1 py-0.5 hover:bg-slate-900/[0.04]">
      <span>
        <span className="block text-[13px] text-slate-800">{label}</span>
        <span className="block text-[11px] text-slate-400">{hint}</span>
      </span>
      <input
        type="checkbox"
        checked={checked}
        onChange={(e) => onToggle(e.target.checked)}
        className="h-4 w-4 accent-[#007aff]"
      />
    </label>
  );
}

export function SettingsDialog() {
  const open = useStore((s) => s.settingsOpen);
  const settings = useStore((s) => s.settings);
  const updateSettings = useStore((s) => s.updateSettings);
  const setSettingsOpen = useStore((s) => s.setSettingsOpen);
  const apiVersion = useStore((s) => s.apiVersion);

  if (!open) return null;

  const close = () => setSettingsOpen(false);
  const labelCls = "mb-1 block text-[12px] font-medium text-slate-600";

  return (
    <div
      className="fixed inset-0 z-[80] grid place-items-center bg-black/40"
      onMouseDown={close}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Settings"
        className="w-[440px] max-w-[94vw] overflow-hidden rounded-xl border border-slate-300/70 bg-white shadow-[0_24px_70px_rgba(0,0,0,0.25)]"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex h-11 shrink-0 items-center justify-between border-b border-slate-200/80 px-4">
          <span className="text-[15px] font-semibold text-slate-900">
            Settings
          </span>
          <button
            className="grid h-7 w-7 place-items-center rounded-md text-slate-400 hover:bg-slate-900/[0.06] hover:text-slate-600"
            title="Close"
            onClick={close}
          >
            <X size={14} strokeWidth={1.7} />
          </button>
        </div>

        <div className="max-h-[70vh] space-y-5 overflow-y-auto p-4">
          <Section title="Appearance">
            <div>
              <span className={labelCls}>Theme</span>
              {THEMES.map((t) => (
                <RadioRow
                  key={t.value}
                  name="settings-theme"
                  value={t.value}
                  checked={settings.theme === t.value}
                  label={t.label}
                  hint={t.hint}
                  onPick={(theme) => updateSettings({ theme })}
                />
              ))}
            </div>
            <div>
              <span className={labelCls}>Font size</span>
              {FONT_SIZES.map((f) => (
                <RadioRow
                  key={f.value}
                  name="settings-font-size"
                  value={f.value}
                  checked={settings.fontSize === f.value}
                  label={f.label}
                  hint={f.hint}
                  onPick={(fontSize) => updateSettings({ fontSize })}
                />
              ))}
            </div>
            <div>
              <span className={labelCls}>Density</span>
              {DENSITIES.map((d) => (
                <RadioRow
                  key={d.value}
                  name="settings-density"
                  value={d.value}
                  checked={settings.density === d.value}
                  label={d.label}
                  hint={d.hint}
                  onPick={(density) => updateSettings({ density })}
                />
              ))}
            </div>
          </Section>

          <Section title="Editor">
            <div className="grid grid-cols-[130px_1fr] items-center gap-3 px-1">
              <label className={labelCls}>Tab size</label>
              <input
                type="number"
                min={1}
                max={8}
                value={settings.tabSize}
                onChange={(e) => {
                  const n = Math.floor(Number(e.target.value));
                  if (Number.isFinite(n) && n >= 1 && n <= 8)
                    updateSettings({ tabSize: n });
                }}
                className="h-7 w-20 rounded-md border border-slate-300 bg-white px-2 text-[13px] focus:ring-2 focus:ring-accent/40 focus:outline-none"
              />
            </div>
            <ToggleRow
              label="Format on save"
              hint="Stored only — not enforced by the editor yet (TODO)"
              checked={settings.formatOnSave}
              onToggle={(formatOnSave) => updateSettings({ formatOnSave })}
            />
          </Section>

          <Section title="Data">
            <ToggleRow
              label="Confirm unsafe writes"
              hint="Warn before UPDATE / DELETE without WHERE"
              checked={settings.confirmUnsafe}
              onToggle={(confirmUnsafe) => updateSettings({ confirmUnsafe })}
            />
          </Section>

          <Section title="About">
            <p className="px-1 text-[13px] text-slate-600">
              MieoSQL backend {apiVersion || "…"}
            </p>
          </Section>
        </div>
      </div>
    </div>
  );
}
