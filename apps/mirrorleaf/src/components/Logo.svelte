<script lang="ts">
  // The app icon, "Resting leaf" in Oat & Moss (owner, 7 Nov 2026; sources in
  // src-tauri/icon-source). Brand colours: the same in light and dark.
  let { size = 64 }: { size?: number } = $props();
</script>

<svg width={size} height={size} viewBox="0 0 100 100" aria-hidden="true">
  <rect width="100" height="100" rx="23" fill="#DFCFAE" />
  <rect x="27" y="23" width="46" height="58" rx="6" fill="#FBF6EC" />
  <path d="M27 25Q27 19 33 19H67Q73 19 73 25V30H27Z" fill="#C4842C" />
  <path
    d="M35 41H59M35 51H55M35 61H47"
    fill="none"
    stroke="#3A2F25"
    stroke-opacity="0.26"
    stroke-width="3"
    stroke-linecap="round"
  />
  <path d="M47 69L40 76" fill="none" stroke="#4E6A35" stroke-width="3.2" stroke-linecap="round" />
  <path d="M47 69A40 40 0 0 1 85 31A40 40 0 0 1 47 69Z" fill="#4E6A35" />
  <path
    d="M51.5 64.5L80.5 35.5"
    fill="none"
    stroke="#FBF6EC"
    stroke-width="1.7"
    stroke-linecap="round"
  />
</svg>
