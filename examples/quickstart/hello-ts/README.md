# HTTP Hello World in TypeScript

This is a simple JavaScript ([TypeScript][ts]) Wasm example that responds with a "Hello World" message for each request. 

[ts]: https://www.typescriptlang.org/

## Prerequisites

- [Wasm Shell (`wash`)](https://wasmcloud.com/docs/v2.0.0-rc/wash/) v2.0.0-rc.6
- [Node Package Manager (`npm`)][npm]
- [NodeJS runtime][nodejs]

[node]: https://nodejs.org
[npm]: https://github.com/npm/cli

## Local development

To get started developing this example quickly, clone the repo and run `wash dev`:

```shell
wash dev
```

`wash dev` does many things for you:

- Builds this project (including necessary `npm` script targets)
- Runs the component locally, exposing the application at `localhost:8000`
- Watches your code for changes and re-deploys when necessary.

### Send a request to the running component

Once `wash dev` is serving your component, send a request to the running component:

```shell
curl localhost:8000
```
```text
Hello from TypeScript!
```

## Build Wasm binary

```bash
wash build
```