import { BUILTIN_CONFIG_PATH, type ConfigEntry } from "../types";

interface ConfigSelectProps {
  id: string;
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
  configs: ConfigEntry[];
  /** Base dir of the session/draft, used to derive the "Base dir (...)" group label and relative option text. */
  baseDir?: string;
  className?: string;
}

const DEFAULT_CLASS_NAME =
  "w-full bg-gray-50 dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm text-gray-800 dark:text-gray-200 focus:border-blue-500 outline-none disabled:opacity-50";

function dirOf(path: string): string {
  const idx = path.lastIndexOf("/");
  return idx === -1 ? path : path.slice(0, idx);
}

function optionLabel(entry: ConfigEntry, baseDir: string | undefined): string {
  const normalizedBaseDir = baseDir?.replace(/\/+$/, "");
  const prefix = normalizedBaseDir ? `${normalizedBaseDir}/` : "";
  const name = prefix && entry.path.startsWith(prefix) ? entry.path.slice(prefix.length) : entry.name;
  return entry.description ? `${name} — ${entry.description}` : name;
}

function renderOptions(entries: ConfigEntry[], baseDir: string | undefined) {
  return entries.map((c) => (
    <option key={c.path} value={c.path}>
      {optionLabel(c, baseDir)}
    </option>
  ));
}

function renderGroup(entries: ConfigEntry[], label: string, baseDir: string | undefined) {
  return entries.length > 0 ? <optgroup label={label}>{renderOptions(entries, baseDir)}</optgroup> : null;
}

/**
 * Shared `<select>` for choosing a workflow config file or the built-in default,
 * grouping file options by discovery source (`local` under the base dir, `user` under the user workflow dir,
 * anything without a `source` under "Other").
 */
export function ConfigSelect({
  id,
  value,
  onChange,
  disabled = false,
  configs,
  baseDir,
  className = DEFAULT_CLASS_NAME,
}: ConfigSelectProps): React.ReactElement {
  const localEntries = configs.filter((c) => c.source === "local");
  const userEntries = configs.filter((c) => c.source === "user");
  const otherEntries = configs.filter((c) => c.source !== "local" && c.source !== "user");

  return (
    <select
      id={id}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      disabled={disabled}
      className={className}
    >
      <option value="">Auto (base dir / user workflows / builtin)</option>
      <option value={BUILTIN_CONFIG_PATH}>Built-in default</option>
      {renderGroup(localEntries, `Base dir (${baseDir || "."})`, baseDir)}
      {renderGroup(
        userEntries,
        `User workflows (${dirOf(userEntries[0]?.path ?? "")})`,
        baseDir,
      )}
      {renderGroup(otherEntries, "Other", baseDir)}
    </select>
  );
}
