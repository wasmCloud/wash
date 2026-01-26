/** @module Interface local:spidermonkey-embedding-splicer/splicer **/
export function stubWasi(engine: Uint8Array, features: Array<Feature>, witWorld: string | undefined, witPath: string | undefined, worldName: string | undefined): Uint8Array;
export function spliceBindings(spidermonkeyEngine: Uint8Array, features: Array<Feature>, witWorld: string | undefined, witPath: string | undefined, worldName: string | undefined, debug: boolean): SpliceResult;
/**
 * # Variants
 * 
 * ## `"i32"`
 * 
 * ## `"i64"`
 * 
 * ## `"f32"`
 * 
 * ## `"f64"`
 */
export type CoreTy = 'i32' | 'i64' | 'f32' | 'f64';
/**
 * # Variants
 * 
 * ## `"stdio"`
 * 
 * ## `"clocks"`
 * 
 * ## `"random"`
 * 
 * ## `"http"`
 * 
 * ## `"fetch-event"`
 */
export type Feature = 'stdio' | 'clocks' | 'random' | 'http' | 'fetch-event';
export interface CoreFn {
  params: Array<CoreTy>,
  ret?: CoreTy,
  retptr: boolean,
  retsize: number,
  paramptr: boolean,
}
export interface SpliceResult {
  wasm: Uint8Array,
  jsBindings: string,
  exports: Array<[string, CoreFn]>,
  imports: Array<[string, string, number]>,
}
