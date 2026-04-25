import Editor from "@monaco-editor/react";

export default function YamlEditor({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return (
    <Editor
      height="60vh"
      language="yaml"
      theme="vs-dark"
      value={value}
      onChange={(v) => onChange(v ?? "")}
      options={{ minimap: { enabled: false }, tabSize: 2 }}
    />
  );
}
