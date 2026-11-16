import { mount } from 'svelte';
import './app.css';
import { applyTheme, readTheme } from './lib/theme';
import App from './App.svelte';

applyTheme(readTheme());

export default mount(App, { target: document.getElementById('app')! });
