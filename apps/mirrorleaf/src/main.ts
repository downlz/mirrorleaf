import { mount } from 'svelte';
import App from './App.svelte';
import { startAura } from './lib/aura';
import './styles.css';

// Before the first paint, so the screen opens with its glow.
startAura();

const target = document.getElementById('app');
if (!target) throw new Error('no #app element');
export default mount(App, { target });
