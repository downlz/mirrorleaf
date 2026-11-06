#!/usr/bin/env bash
# Writes the Android app's Firebase configuration for a CI build.
#
# The real google-services.json stays out of the public repo. With the
# GOOGLE_SERVICES_JSON secret set, CI uses it; without it (forks, or before the
# secret exists), a placeholder lets the app build, though it can't receive
# pushes.
set -euo pipefail

target=apps/mirrorleaf/src-tauri/gen/android/app/google-services.json
if [ -n "${GOOGLE_SERVICES_JSON:-}" ]; then
  printf '%s' "$GOOGLE_SERVICES_JSON" > "$target"
else
  cat > "$target" <<'JSON'
{
  "project_info": {
    "project_number": "000000000000",
    "project_id": "mirrorleaf-ci",
    "storage_bucket": "mirrorleaf-ci.appspot.com"
  },
  "client": [
    {
      "client_info": {
        "mobilesdk_app_id": "1:000000000000:android:0000000000000000",
        "android_client_info": { "package_name": "app.mirrorleaf" }
      },
      "api_key": [{ "current_key": "ci-placeholder" }]
    }
  ],
  "configuration_version": "1"
}
JSON
fi
