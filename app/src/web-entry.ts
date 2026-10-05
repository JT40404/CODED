/**
 * Browser bundle for the website's Launch page. Built by build-web.cjs into
 * public/coded-launch.js; the IDL is served from /coded_router.json.
 *
 * The page calls window.CODED.connect() and window.CODED.launch(params).
 */
import { Connection, PublicKey, VersionedTransaction } from "@solana/web3.js";
import type { Idl } from "@coral-xyz/anchor";
import { codedProgram } from "./coded";
import { launchToken, validateLaunch, type LaunchParams, type WalletLike } from "./launch";
import { uploadViaEndpoint } from "./metadata";

interface InjectedWallet {
  publicKey: PublicKey | null;
  connect(): Promise<{ publicKey: PublicKey }>;
  signAllTransactions(txs: VersionedTransaction[]): Promise<VersionedTransaction[]>;
}

declare global {
  interface Window {
    solana?: InjectedWallet;
    CODED?: unknown;
    CODED_CONFIG?: Partial<{ rpc: string; metadataEndpoint: string; idlUrl: string }>;
  }
}

let wallet: WalletLike | null = null;

window.CODED = {
  validate: validateLaunch,
  async connect(): Promise<string> {
    const w = window.solana;
    if (!w) throw new Error("No Solana wallet found. Install Phantom, Solflare or Backpack.");
    const { publicKey } = await w.connect();
    wallet = { publicKey, signAllTransactions: (txs) => w.signAllTransactions(txs) };
    return publicKey.toBase58();
  },
  async launch(params: LaunchParams, onStatus: (m: string) => void) {
    if (!wallet) throw new Error("Connect a wallet first.");
    const cfg = {
      rpc: `${location.origin}/api/rpc`,
      metadataEndpoint: "/api/metadata",
      idlUrl: "/coded_router.json",
      ...window.CODED_CONFIG,
    };
    // web3.js needs an absolute URL; confirmations are polled, so no websocket is needed.
    const connection = new Connection(new URL(cfg.rpc, location.origin).toString(), {
      commitment: "confirmed",
      disableRetryOnRateLimit: false,
    });
    const idlRes = await fetch(cfg.idlUrl);
    if (!idlRes.ok) throw new Error("The CODED program isn't deployed yet, so launching is off for now.");
    const idl = (await idlRes.json()) as Idl;
    const res = await launchToken({
      connection,
      wallet,
      program: codedProgram(connection, idl),
      params,
      uploadMetadata: uploadViaEndpoint(cfg.metadataEndpoint),
      onStatus,
    });
    return {
      mint: res.mint.toBase58(),
      router: res.router?.toBase58() ?? null,
      vault: res.vault?.toBase58() ?? null,
      signatures: res.signatures,
    };
  },
};
