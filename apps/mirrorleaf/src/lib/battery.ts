// Where each phone maker keeps the setting that lets an app run in the
// background (M12, E8). Guidance only: asking Android for the exemption
// directly needs a permission Google Play restricts.

/** The steps for a maker (Build.MANUFACTURER, lower case). */
export function batterySteps(maker: string): string {
  const m = maker.toLowerCase();
  if (m.includes('samsung')) {
    return 'Settings → Apps → Mirrorleaf → Battery → Unrestricted. Also check that Mirrorleaf isn’t under Battery → Background usage limits → Sleeping apps.';
  }
  if (['xiaomi', 'redmi', 'poco'].some((name) => m.includes(name))) {
    return 'Settings → Apps → Manage apps → Mirrorleaf: turn Autostart on, and set Battery saver to No restrictions.';
  }
  if (['oneplus', 'oppo', 'realme'].some((name) => m.includes(name))) {
    return 'Settings → Apps → Mirrorleaf → Battery usage: allow background activity, and auto-launch where it’s offered.';
  }
  if (['vivo', 'iqoo'].some((name) => m.includes(name))) {
    return 'Settings → Battery → Background power consumption management → Mirrorleaf → Allow.';
  }
  if (['huawei', 'honor'].some((name) => m.includes(name))) {
    return 'Settings → Battery → App launch → Mirrorleaf → Manage manually, with all three switches on.';
  }
  return 'Settings → Apps → Mirrorleaf → App battery usage → Unrestricted.';
}
