import { describe, expect, it } from 'vitest';
import { batterySteps } from './battery';

describe('battery guidance', () => {
  it('gives each maker its own steps, and plain Android otherwise', () => {
    expect(batterySteps('samsung')).toContain('Sleeping apps');
    expect(batterySteps('Xiaomi')).toContain('Autostart');
    expect(batterySteps('POCO')).toContain('Autostart');
    expect(batterySteps('oneplus')).toContain('background activity');
    expect(batterySteps('motorola')).toBe(
      'Settings → Apps → Mirrorleaf → App battery usage → Unrestricted.',
    );
  });
});
