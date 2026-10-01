// Typed fetch wrappers for the server's `/api` routes.

export interface Health {
  ok: boolean
}

export async function getHealth(): Promise<Health> {
  const res = await fetch('/api/health')
  if (!res.ok) throw new Error(`GET /api/health failed: ${res.status}`)
  return res.json()
}
