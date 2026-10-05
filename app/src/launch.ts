/**
 * One-click launcher: creates a pump.fun coin and locks its creator fees into
 * the routes the creator chose. Runs in the browser with any wallet adapter
 * that supports signAllTransactions.
 *
 *   tx1  create_v2 (+ optional dev buy)                     signed by wallet + mint
 *   tx2  coded initialize_router + pump create_fee_sharing_config
 *   tx3  pump update_fee_shares_v2  (permanent: admin is revoked)
 *
 * All three are signed in one wallet prompt and sent in order. For full
 * atomicity send them as a Jito bundle instead (see README).
 */
import BN from "bn.js";
import {
  ComputeBudgetProgram,
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  TransactionInstruction,
  TransactionMessage,
  VersionedTransaction,
} from "@solana/web3.js";
import { NATIVE_MINT, TOKEN_PROGRAM_ID } from "@solana/spl-token";
import {
  OnlinePumpSdk,
  PUMP_SDK,
  bondingCurvePda,
  getBuyTokenAmountFromSolAmount,
  type Shareholder,
} from "@pump-fun/pump-sdk";
import type { Program } from "@coral-xyz/anchor";

import { BPS, LP_MODE_BURN, LP_MODE_LOCK, VENUE_BONDING_CURVE, globalPda, routerPda, vaultPda } from "./constants";

export interface CustomRoute {
  address: string;
  bps: number;
  label?: string;
}

export interface LaunchParams {
  name: string;
  symbol: string;
  description: string;
  image: Blob;
  website?: string;
  twitter?: string;
  telegram?: string;
  /** SOL spent on the creator's first buy. 0 to skip. */
  devBuySol: number;
  /** Shares of all creator fees, in bps. Must total 10000. */
  split: {
    creatorBps: number;
    holdersBps: number;
    lpBps: number;
    burnBps: number;
    custom: CustomRoute[];
  };
  /** Where the creator share goes. Defaults to the launching wallet. */
  creatorPayout?: string;
  rules: {
    /** Minimum balance in whole tokens (6 decimals applied here). */
    minHolderTokens: number;
    maxSlippageBps: number;
    lpMode: "burn" | "lock";
    claimIntervalSecs: number;
    excludeCreator: boolean;
  };
  priorityMicroLamports?: number;
}

export interface WalletLike {
  publicKey: PublicKey;
  signAllTransactions(txs: VersionedTransaction[]): Promise<VersionedTransaction[]>;
}

export type UploadMetadata = (input: {
  name: string;
  symbol: string;
  description: string;
  image: Blob;
  website?: string;
  twitter?: string;
  telegram?: string;
}) => Promise<string>;

export interface LaunchResult {
  mint: PublicKey;
  router: PublicKey | null;
  vault: PublicKey | null;
  signatures: string[];
}

export function validateLaunch(p: LaunchParams): string[] {
  const errs: string[] = [];
  if (!p.name.trim() || p.name.length > 32) errs.push("Name must be 1 to 32 characters.");
  if (!p.symbol.trim() || p.symbol.length > 13) errs.push("Ticker must be 1 to 13 characters.");
  const s = p.split;
  const total = s.creatorBps + s.holdersBps + s.lpBps + s.burnBps + s.custom.reduce((a, c) => a + c.bps, 0);
  if (total !== BPS) errs.push(`Fee split must total 100%. It totals ${total / 100}%.`);
  const recipients = (s.creatorBps > 0 ? 1 : 0) + (s.holdersBps + s.lpBps + s.burnBps > 0 ? 1 : 0) + s.custom.length;
  if (recipients > 10) errs.push("pump.fun allows at most 10 fee recipients.");
  for (const c of s.custom) {
    if (c.bps <= 0) errs.push("Each custom route needs a share above 0%.");
    try { new PublicKey(c.address); } catch { errs.push(`Custom route address is not valid: ${c.address}`); }
  }
  if (p.rules.maxSlippageBps < 10 || p.rules.maxSlippageBps > 1000) errs.push("Max slippage must be between 0.1% and 10%.");
  if (p.rules.claimIntervalSecs < 900) errs.push("Claim interval must be at least 15 minutes.");
  if (p.devBuySol < 0) errs.push("Dev buy can't be negative.");
  return errs;
}

/** Splits the CODED-handled share into the router's internal ratios. */
export function routerRatios(s: LaunchParams["split"]) {
  const coded = s.holdersBps + s.lpBps + s.burnBps;
  if (coded === 0) return null;
  const holders = Math.round((s.holdersBps * BPS) / coded);
  const lp = Math.round((s.lpBps * BPS) / coded);
  return { codedBps: coded, holders, lp, burn: BPS - holders - lp };
}

async function toTx(
  connection: Connection,
  payer: PublicKey,
  ixs: TransactionInstruction[],
  cu: number,
  priority: number,
): Promise<VersionedTransaction> {
  const { blockhash } = await connection.getLatestBlockhash("confirmed");
  const msg = new TransactionMessage({
    payerKey: payer,
    recentBlockhash: blockhash,
    instructions: [
      ComputeBudgetProgram.setComputeUnitLimit({ units: cu }),
      ComputeBudgetProgram.setComputeUnitPrice({ microLamports: priority }),
      ...ixs,
    ],
  }).compileToV0Message();
  return new VersionedTransaction(msg);
}

/** True if the transaction fits Solana's 1232-byte packet limit. */
function fits(tx: VersionedTransaction): boolean {
  try {
    return tx.serialize().length <= 1232;
  } catch {
    return false;
  }
}

/** Polls for confirmation over plain HTTP (works through the /api/rpc proxy). */
async function confirm(connection: Connection, sig: string, timeoutMs = 90_000): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    const { value } = await connection.getSignatureStatuses([sig]);
    const st = value[0];
    if (st?.err) throw new Error(`Transaction failed: ${JSON.stringify(st.err)}`);
    if (st && (st.confirmationStatus === "confirmed" || st.confirmationStatus === "finalized")) return;
    await new Promise((r) => setTimeout(r, 1500));
  }
  throw new Error(`Not confirmed after ${timeoutMs / 1000}s. Check ${sig} on an explorer before retrying.`);
}

export async function launchToken(args: {
  connection: Connection;
  wallet: WalletLike;
  program: Program;
  params: LaunchParams;
  uploadMetadata: UploadMetadata;
  onStatus?: (msg: string) => void;
}): Promise<LaunchResult> {
  const { connection, wallet, program, params: p, uploadMetadata } = args;
  const status = args.onStatus ?? (() => {});
  const errs = validateLaunch(p);
  if (errs.length) throw new Error(errs.join(" "));

  const user = wallet.publicKey;
  const priority = p.priorityMicroLamports ?? 200_000;

  status("Uploading image and metadata");
  const uri = await uploadMetadata({
    name: p.name,
    symbol: p.symbol,
    description: p.description,
    image: p.image,
    website: p.website,
    twitter: p.twitter,
    telegram: p.telegram,
  });
  if (uri.length > 200) throw new Error("Metadata URI is longer than 200 characters.");

  const mintKp = Keypair.generate();
  const mint = mintKp.publicKey;
  const pump = new OnlinePumpSdk(connection);

  // ---------------------------------------------------------------- tx1
  status("Building coin creation");
  const createOnly = await PUMP_SDK.createV2Instruction({
    mint,
    name: p.name,
    symbol: p.symbol,
    uri,
    creator: user,
    user,
    mayhemMode: false,
  });
  let createIxs: TransactionInstruction[] = [createOnly];
  let devBuyLater = false;
  const devBuyLamports = new BN(Math.floor(p.devBuySol * 1e9));
  if (p.devBuySol > 0) {
    const [global, feeConfig] = await Promise.all([pump.fetchGlobal(), pump.fetchFeeConfig()]);
    const amount = getBuyTokenAmountFromSolAmount({
      global,
      feeConfig,
      mintSupply: null,
      bondingCurve: null,
      amount: devBuyLamports,
      quoteMint: NATIVE_MINT,
    });
    const withBuy = await PUMP_SDK.createV2AndBuyInstructions({
      global,
      mint,
      name: p.name,
      symbol: p.symbol,
      uri,
      creator: user,
      user,
      amount,
      // Max SOL the first buy may spend: 2% headroom.
      solAmount: devBuyLamports.muln(102).divn(100),
      mayhemMode: false,
    });
    const probe = await toTx(connection, user, withBuy, 500_000, priority);
    probe.sign([mintKp]);
    if (fits(probe)) createIxs = withBuy;
    else devBuyLater = true; // too big for one transaction; buy right after
  }

  // ---------------------------------------------------------------- tx2/tx3
  status("Building fee routing");
  const ratios = routerRatios(p.split);
  const creatorPayout = new PublicKey(p.creatorPayout ?? user.toBase58());
  let router: PublicKey | null = null;
  let vault: PublicKey | null = null;

  const shareholders = new Map<string, number>();
  const add = (k: PublicKey, bps: number) => {
    if (bps <= 0) return;
    shareholders.set(k.toBase58(), (shareholders.get(k.toBase58()) ?? 0) + bps);
  };

  const routerIxs: TransactionInstruction[] = [];
  if (ratios) {
    router = routerPda(mint, user);
    vault = vaultPda(router);
    const bc = bondingCurvePda(mint);
    routerIxs.push(
      await program.methods
        .initializeRouter(
          {
            holdersBps: ratios.holders,
            lpBps: ratios.lp,
            burnBps: ratios.burn,
            minHolderBalance: new BN(Math.floor(p.rules.minHolderTokens)).mul(new BN(1_000_000)),
            maxSlippageBps: p.rules.maxSlippageBps,
            lpMode: p.rules.lpMode === "lock" ? LP_MODE_LOCK : LP_MODE_BURN,
            claimIntervalSecs: new BN(p.rules.claimIntervalSecs),
            excludeCreator: p.rules.excludeCreator,
          },
          VENUE_BONDING_CURVE,
        )
        .accountsPartial({
          authority: user,
          global: globalPda(),
          mint,
          router,
          vault,
          venueA: bc,
          venueB: bc,
          venueC: bc,
          systemProgram: SystemProgram.programId,
        })
        .instruction(),
    );
    add(vault, ratios.codedBps);
  }
  add(creatorPayout, p.split.creatorBps);
  for (const c of p.split.custom) add(new PublicKey(c.address), c.bps);

  const newShareholders: Shareholder[] = [...shareholders.entries()].map(([address, shareBps]) => ({
    address: new PublicKey(address),
    shareBps,
  }));

  // If 100% goes to the launching wallet there is nothing to configure.
  const needsSharing = !(newShareholders.length === 1 && newShareholders[0].address.equals(user));

  const txs: VersionedTransaction[] = [];
  txs.push(await toTx(connection, user, createIxs, 500_000, priority));
  if (needsSharing) {
    const createSharing = await PUMP_SDK.createFeeSharingConfig({ creator: user, mint, pool: null });
    txs.push(await toTx(connection, user, [...routerIxs, createSharing], 400_000, priority));
    const update = await PUMP_SDK.updateFeeSharesV2({
      authority: user,
      mint,
      currentShareholders: [user],
      newShareholders,
      quoteMint: NATIVE_MINT,
      quoteTokenProgram: TOKEN_PROGRAM_ID,
    });
    txs.push(await toTx(connection, user, [update], 400_000, priority));
  }

  txs[0].sign([mintKp]);

  status("Waiting for wallet approval");
  const signed = await wallet.signAllTransactions(txs);

  const signatures: string[] = [];
  for (let i = 0; i < signed.length; i++) {
    status(i === 0 ? "Creating coin" : i === 1 ? "Setting up fee routing" : "Locking the fee split");
    const sig = await connection.sendTransaction(signed[i], { skipPreflight: false, maxRetries: 3 });
    await confirm(connection, sig);
    signatures.push(sig);
  }

  if (devBuyLater) {
    status("Building dev buy");
    const state = await pump.fetchBuyState(mint, user);
    const [global, feeConfig] = await Promise.all([pump.fetchGlobal(), pump.fetchFeeConfig()]);
    const amount = getBuyTokenAmountFromSolAmount({
      global,
      feeConfig,
      mintSupply: state.bondingCurve.tokenTotalSupply,
      bondingCurve: state.bondingCurve,
      amount: devBuyLamports,
      quoteMint: NATIVE_MINT,
    });
    const buyIxs = await PUMP_SDK.buyV2Instructions({
      global,
      bondingCurveAccountInfo: state.bondingCurveAccountInfo,
      bondingCurve: state.bondingCurve,
      associatedUserAccountInfo: state.associatedUserAccountInfo,
      mint,
      user,
      amount,
      quoteAmount: devBuyLamports,
      slippage: 2,
      quoteTokenProgram: state.quoteTokenProgram,
    });
    status("Waiting for wallet approval (dev buy)");
    const [buyTx] = await wallet.signAllTransactions([await toTx(connection, user, buyIxs, 300_000, priority)]);
    const sig = await connection.sendTransaction(buyTx, { maxRetries: 3 });
    await confirm(connection, sig);
    signatures.push(sig);
  }

  status("Launched");
  return { mint, router, vault, signatures };
}
