import { mount } from 'svelte'
// Weight, width and optical size axes in one file, served from this origin.
import '@fontsource-variable/bricolage-grotesque/standard.css'
import './styles/tokens.css'
import './styles/base.css'
import App from './App.svelte'
import { routeFor } from './lib/sections'

// The one place where the address decides what the page is: `/admin` and
// everything under it is the admin page, and the game is never mounted
// there; every other address is the game, opened on the section it names
// (`/`, `/pop`, `/rock`, `/hip-hop`; anything unknown is General).
const target = document.getElementById('app')!
const route = routeFor(location.pathname)

if (route.page === 'admin') {
  // Fetched only here, so the admin page stays out of the player's bundle.
  const { default: Admin } = await import('./Admin.svelte')
  mount(Admin, { target })
} else {
  // An unknown or oddly written address becomes the section's own.
  if (location.pathname !== route.path) {
    history.replaceState(null, '', route.path + location.search + location.hash)
  }
  mount(App, { target, props: { section: route.section } })
}
