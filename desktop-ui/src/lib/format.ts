export function formatBoolean(value: boolean): string {
  return value ? "Yes" : "No";
}

export function formatNullable(value: string | number | null | undefined): string {
  if (value === null || value === undefined || value === "") {
    return "Unavailable";
  }

  return String(value);
}

export function titleCase(input: string): string {
  return input
    .replace(/([A-Z])/g, " $1")
    .replace(/[-_]/g, " ")
    .trim()
    .replace(/\b\w/g, (char) => char.toUpperCase());
}
