// presets.json is read as plain data and checked by schema.ts; the app's
// tsconfig has no resolveJsonModule, so the import gets this type.
declare module "*/presets.json" {
  const value: unknown;
  export default value;
}
