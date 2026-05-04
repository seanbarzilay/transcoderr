export function basename(path: string): string {
  const last = path.split(/[\\/]/).filter(Boolean).pop();
  return last ?? path;
}
