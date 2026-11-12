<script lang="ts" module>
  export interface MenuItem {
    label: string;
    action?: () => void;
    danger?: boolean;
    disabled?: boolean;
    checked?: boolean;
    /** A heading above the items that follow. */
    section?: boolean;
    /** How far in to set the label: a folder's level in "Move to" (M12). */
    depth?: number;
  }
</script>

<script lang="ts">
  // A small popover menu, opened at a point (a right-click or a long press) or
  // under a button. Big enough to tap on a tablet.
  let { items, x, y, onclose }: { items: MenuItem[]; x: number; y: number; onclose: () => void } =
    $props();

  let menu: HTMLDivElement | undefined = $state();
  let left = $state(0);
  let top = $state(0);

  $effect(() => {
    if (!menu) return;
    const { offsetWidth: w, offsetHeight: h } = menu;
    left = Math.max(8, Math.min(x, window.innerWidth - w - 8));
    top = Math.max(8, y + h > window.innerHeight - 8 ? y - h : y);
    menu.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus();
  });

  function choose(item: MenuItem) {
    onclose();
    item.action?.();
  }

  function keydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.stopPropagation();
      onclose();
      return;
    }
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
    event.preventDefault();
    const buttons = Array.from(
      menu?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? [],
    );
    const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const next = event.key === 'ArrowDown' ? at + 1 : at - 1;
    buttons[(next + buttons.length) % buttons.length]?.focus();
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="scrim clear"
  onpointerdown={onclose}
  oncontextmenu={(e) => {
    e.preventDefault();
    onclose();
  }}
></div>
<div
  class="menu"
  role="menu"
  tabindex="-1"
  bind:this={menu}
  style:left="{left}px"
  style:top="{top}px"
  onkeydown={keydown}
>
  {#each items as item, i (i)}
    {#if item.section}
      <div class="menu-section">{item.label}</div>
    {:else}
      <button
        type="button"
        role="menuitem"
        class:danger={item.danger}
        disabled={item.disabled}
        onclick={() => choose(item)}
      >
        <span class="menu-check">{item.checked ? '✓' : ''}</span><span
          style:padding-left={item.depth ? `${item.depth * 14}px` : undefined}>{item.label}</span
        >
      </button>
    {/if}
  {/each}
</div>
