<script lang="ts">
  // The four-digit number both devices show while one joins (E1).
  let { value, small = false }: { value: string; small?: boolean } = $props();
</script>

<div class="number" class:small aria-label={value.split('').join(' ')}>
  {#each value.split('') as digit, i (i)}
    <span>{digit}</span>
  {/each}
</div>
