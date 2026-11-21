declare module "node:test" {
  export function describe(
    name: string,
    fn: () => void | Promise<void>
  ): Promise<void>;
  export function it(
    name: string,
    fn: () => void | Promise<void>
  ): Promise<void>;
  export function test(
    name: string,
    fn: () => void | Promise<void>
  ): Promise<void>;
}

declare module "node:assert/strict" {
  const assert: {
    equal<T>(actual: T, expected: T, message?: string): void;
    notEqual<T>(actual: T, expected: T, message?: string): void;
    deepEqual(actual: unknown, expected: unknown, message?: string): void;
    ok(value: unknown, message?: string): asserts value;
  };
  export default assert;
  export function equal<T>(actual: T, expected: T, message?: string): void;
  export function notEqual<T>(actual: T, expected: T, message?: string): void;
  export function deepEqual(
    actual: unknown,
    expected: unknown,
    message?: string
  ): void;
  export function ok(value: unknown, message?: string): asserts value;
}
