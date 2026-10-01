import { mount } from 'svelte'
// Weight, width and optical size axes in one file, served from this origin.
import '@fontsource-variable/bricolage-grotesque/standard.css'
import './styles/tokens.css'
import './styles/base.css'
import App from './App.svelte'

const app = mount(App, {
  target: document.getElementById('app')!,
})

export default app
