/**
 * Demo mode only: stops the clock at Friday 15:47 of the current week, so screenshots show a full working week with
 * a timer running at a fixed 0:47:00. Does nothing in the normal app.
 */
if (window.focusDemo) {
  const Real = Date;
  const real = new Real();
  const friday = new Real(real.getFullYear(), real.getMonth(), real.getDate() - ((real.getDay() + 6) % 7) + 4, 15, 47, 0);
  // Frozen, not shifted: the running timer reads the same every time (0:47:00), so screenshots match.
  const frozen = friday.getTime();
  class DemoDate extends Real {
    constructor(...args: unknown[]) {
      if (args.length === 0) super(frozen);
      else super(...(args as [number]));
    }
    static now() {
      return frozen;
    }
  }
  globalThis.Date = DemoDate as DateConstructor;
}
export {};
