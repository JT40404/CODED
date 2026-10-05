// Proxies the browser's Solana RPC calls so your RPC key stays server-side.
// Only the methods the launcher needs are allowed.
const ALLOWED = new Set([
  "getLatestBlockhash",
  "getAccountInfo",
  "getMultipleAccounts",
  "getBalance",
  "getTokenAccountBalance",
  "getMinimumBalanceForRentExemption",
  "getSignatureStatuses",
  "simulateTransaction",
  "sendTransaction",
  "getFeeForMessage",
  "getEpochInfo",
  "getBlockHeight",
  "getSlot",
]);

type RpcCall = { method?: string };

export async function POST(request: Request): Promise<Response> {
  const upstream = process.env.RPC_URL;
  if (!upstream) return Response.json({ error: "RPC_URL is not set on the server." }, { status: 500 });

  const text = await request.text();
  let body: RpcCall | RpcCall[];
  try {
    body = JSON.parse(text);
  } catch {
    return Response.json({ error: "Request body must be JSON-RPC." }, { status: 400 });
  }
  const calls = Array.isArray(body) ? body : [body];
  if (calls.length > 20 || calls.some((c) => !c.method || !ALLOWED.has(c.method))) {
    return Response.json({ error: "RPC method not allowed." }, { status: 403 });
  }

  const res = await fetch(upstream, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: text,
  });
  return new Response(res.body, {
    status: res.status,
    headers: { "content-type": "application/json" },
  });
}
