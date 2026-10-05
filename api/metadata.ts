// Pins the coin image and pump.fun metadata JSON to IPFS via Pinata.
// Set PINATA_JWT in the Vercel project's environment variables.
import { handleMetadataUpload } from "../app/src/metadata";

export async function POST(request: Request): Promise<Response> {
  const jwt = process.env.PINATA_JWT;
  if (!jwt) return Response.json({ error: "PINATA_JWT is not set on the server." }, { status: 500 });
  try {
    return await handleMetadataUpload(request, jwt);
  } catch (e) {
    return Response.json({ error: (e as Error).message }, { status: 502 });
  }
}
