// A context menu from a right-click, or from a long press on a touch screen
// (tablets and phones; iPadOS's web view sends no contextmenu event for it).

const LONG_PRESS_MS = 500;
const SLOP_PX = 10;

export function contextMenu(node: HTMLElement, open: (x: number, y: number) => void) {
  let handler = open;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let start: { x: number; y: number } | null = null;
  let pressed = 0;

  const cancel = () => {
    clearTimeout(timer);
    start = null;
  };
  const down = (event: PointerEvent) => {
    if (event.pointerType !== 'touch') return;
    start = { x: event.clientX, y: event.clientY };
    timer = setTimeout(() => {
      if (!start) return;
      pressed = Date.now();
      handler(start.x, start.y);
    }, LONG_PRESS_MS);
  };
  const move = (event: PointerEvent) => {
    if (start && Math.hypot(event.clientX - start.x, event.clientY - start.y) > SLOP_PX) cancel();
  };
  const menu = (event: MouseEvent) => {
    event.preventDefault();
    // Android sends contextmenu after a long press too: open only once.
    if (Date.now() - pressed < 1000) return;
    handler(event.clientX, event.clientY);
  };

  node.addEventListener('pointerdown', down);
  node.addEventListener('pointermove', move);
  node.addEventListener('pointerup', cancel);
  node.addEventListener('pointercancel', cancel);
  node.addEventListener('contextmenu', menu);
  return {
    update(next: (x: number, y: number) => void) {
      handler = next;
    },
    destroy() {
      cancel();
      node.removeEventListener('pointerdown', down);
      node.removeEventListener('pointermove', move);
      node.removeEventListener('pointerup', cancel);
      node.removeEventListener('pointercancel', cancel);
      node.removeEventListener('contextmenu', menu);
    },
  };
}
