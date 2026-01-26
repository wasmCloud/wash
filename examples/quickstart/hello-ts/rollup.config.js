// rollup.config.mjs
import resolve from '@rollup/plugin-node-resolve';
import typescript from '@rollup/plugin-typescript';

export default {
  input: 'src/hello_ts.ts', // Entry point is the TypeScript source
  output: {
    file: 'dist/index.bundled.js', // Output bundled JS file
    format: 'esm', // Output format as ES Module
    sourcemap: true,
  },
  external: [/wasi:.*/], // Exclude WASI imports from bundling
  plugins: [
    typescript({
      // Compile TypeScript first
      tsconfig: './tsconfig.json',
      sourceMap: true,
      inlineSources: true,
    }),
    resolve({
      // Resolve node_modules imports
      preferBuiltins: false, // Important for WASI environment
      browser: true, // Treat as browser environment for dependencies
    }),
  ],
};