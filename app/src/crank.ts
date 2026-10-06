/**
 * CODED crank. Every instruction it sends is permissionless, so anyone can
 * run one; the program checks every outcome on-chain.
 *
 * Per router, each loop:
 *   1. claim     pump transfer + distribute, then coded account_inflows
 *   2. observe   nudge the reference price (rate limited on-chain)
 *   3. burn      buy from the curve or pool with the burn bucket, burn it
 *   4. liquidity buy with half the LP bucket, deposit, burn or lock LP
 *   5. holders   snapshot, post a merkle epoch, push claims, close old epochs
 *
 *   RPC_URL=... CRANK_KEYPAIR=... IDL_PATH=../target/idl/coded_router.json npx tsx src/crank.ts
 */
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import BN from "bn.js";
import { AnchorProvider, Program, Wallet, type Idl } from "@coral-xyz/anchor";
import {
  type AccountMeta,
  ComputeBudgetProgram,
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  TransactionInstruction,
  TransactionMessage,
  VersionedTransaction,
} from "@solana/web3.js";
import {
  ASSOCIATED_TOKEN_PROGRAM_ID,
  NATIVE_MINT,
  TOKEN_2022_PROGRAM_ID,
  TOKEN_PROGRAM_ID,
  createAssociatedTokenAccountIdempotentInstruction,
  getAssociatedTokenAddressSync,
} from "@solana/spl-token";
import {
  OnlinePumpSdk,
  PUMP_SDK,
  bondingCurvePda,
  canonicalPumpPoolPda,
  feeSharingConfigPda,
  getBuyTokenAmountFromSolAmount,
  pumpPoolAuthorityPda,
  userVolumeAccumulatorPda as pumpUserVolumePda,
} from "@pump-fun/pump-sdk";
import {
  OnlinePumpAmmSdk,
  PUMP_AMM_PROGRAM_ID,
  PUMP_AMM_SDK,
  lpMintPda,
  userVolumeAccumulatorPda as ammUserVolumePda,
} from "@pump-fun/pump-swap-sdk";

import {
  BPS,
  PURPOSE_BURN,
  PURPOSE_LP,
  VAULT_RENT_RESERVE,
  VENUE_AMM,
  VENUE_BONDING_CURVE,
  epochPda,
  globalPda,
  vaultPda,
  vaultWsolAta,
} from "./constants";
import { buildTree, proofFor } from "./merkle";
import { allocate, snapshotHolders } from "./holders";

// ------------------------------------------------------------------ config
const RPC = process.env.RPC_URL ?? "https://api.mainnet-beta.solana.com";
const IDL_PATH = process.env.IDL_PATH ?? "../target/idl/coded_router.json";
const DATA_DIR = process.env.DATA_DIR ?? "./data";
const LOOP_SECS = Number(process.env.LOOP_SECS ?? 60);
const PRIORITY = Number(process.env.PRIORITY_MICRO_LAMPORTS ?? 100_000);
const MIN_TRADE = BigInt(process.env.MIN_TRADE_LAMPORTS ?? 5_000_000); // 0.005 SOL
const MIN_EPOCH = BigInt(process.env.MIN_EPOCH_LAMPORTS ?? 50_000_000); // 0.05 SOL
const MIN_PAYOUT = BigInt(process.env.MIN_PAYOUT_LAMPORTS ?? 1_000_000); // 0.001 SOL
const CLAIM_WINDOW_SECS = Number(process.env.CLAIM_WINDOW_SECS ?? 14 * 86_400);
const PUSH_CLAIMS = process.env.PUSH_CLAIMS !== "false";

const connection = new Connection(RPC, "confirmed");
const payer = Keypair.fromSecretKey(
  Uint8Array.from(JSON.parse(readFileSync(process.env.CRANK_KEYPAIR!, "utf8"))),
);
const idl = JSON.parse(readFileSync(IDL_PATH, "utf8")) as Idl;
const program = new Program(idl, new AnchorProvider(connection, new Wallet(payer), { commitment: "confirmed" }));
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const accounts = program.account as any;
const pump = new OnlinePumpSdk(connection);
const amm = new OnlinePumpAmmSdk(connection);

const log = (...a: unknown[]) => console.log(new Date().toISOString(), ...a);
const big = (v: BN | number | bigint) => BigInt(v.toString());

// ------------------------------------------------------------------ tx helpers
async function send(ixs: TransactionInstruction[], label: string, cu = 600_000): Promise<string | null> {
  const { blockhash, lastValidBlockHeight } = await connection.getLatestBlockhash();
  const msg = new TransactionMessage({
    payerKey: payer.publicKey,
    recentBlockhash: blockhash,
    instructions: [
      ComputeBudgetProgram.setComputeUnitLimit({ units: cu }),
      ComputeBudgetProgram.setComputeUnitPrice({ microLamports: PRIORITY }),
      ...ixs,
    ],
  }).compileToV0Message();
  const tx = new VersionedTransaction(msg);
  tx.sign([payer]);
  const sim = await connection.simulateTransaction(tx, { sigVerify: false });
  if (sim.value.err) {
    log(`skip ${label}:`, JSON.stringify(sim.value.err), sim.value.logs?.slice(-4).join(" | "));
    return null;
  }
  const sig = await connection.sendTransaction(tx, { skipPreflight: true, maxRetries: 3 });
  await connection.confirmTransaction({ signature: sig, blockhash, lastValidBlockHeight }, "confirmed");
  log(`${label}: ${sig}`);
  return sig;
}

/** [program, ...accounts] with the vault flagged non-signer at the top level. */
function asRemaining(ix: TransactionInstruction, vault: PublicKey): AccountMeta[] {
  return [
    { pubkey: ix.programId, isSigner: false, isWritable: false },
    ...ix.keys.map((k) => ({
      pubkey: k.pubkey,
      isWritable: k.isWritable,
      isSigner: k.pubkey.equals(vault) ? false : k.isSigner,
    })),
  ];
}

function lastIxFor(ixs: TransactionInstruction[], programId: PublicKey): TransactionInstruction {
  const hits = ixs.filter((i) => i.programId.equals(programId));
  if (!hits.length) throw new Error("SDK returned no instruction for target program");
  return hits[hits.length - 1];
}

// ------------------------------------------------------------------ context
interface Ctx {
  router: PublicKey;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  r: any;
  mint: PublicKey;
  vault: PublicKey;
  vaultWsol: PublicKey;
  vaultToken: PublicKey;
  graduated: boolean;
  bc: PublicKey;
  pool: PublicKey;
  poolBase: PublicKey;
  poolQuote: PublicKey;
  venue: number;
  venueAccounts: { venueA: PublicKey; venueB: PublicKey; venueC: PublicKey };
}

async function loadCtx(router: PublicKey): Promise<Ctx> {
  const r = await accounts.router.fetch(router);
  const mint: PublicKey = r.mint;
  const vault = vaultPda(router);
  const bc = bondingCurvePda(mint);
  const curve = await pump.fetchBondingCurve(mint);
  const pool = canonicalPumpPoolPda(mint);
  const poolBase = getAssociatedTokenAddressSync(mint, pool, true, r.tokenProgram);
  const poolQuote = getAssociatedTokenAddressSync(NATIVE_MINT, pool, true, TOKEN_PROGRAM_ID);
  const graduated = curve.complete;
  return {
    router,
    r,
    mint,
    vault,
    vaultWsol: vaultWsolAta(vault),
    vaultToken: getAssociatedTokenAddressSync(mint, vault, true, r.tokenProgram),
    graduated,
    bc,
    pool,
    poolBase,
    poolQuote,
    venue: graduated ? VENUE_AMM : VENUE_BONDING_CURVE,
    venueAccounts: graduated
      ? { venueA: pool, venueB: poolBase, venueC: poolQuote }
      : { venueA: bc, venueB: bc, venueC: bc },
  };
}

async function reserves(c: Ctx): Promise<{ token: bigint; quote: bigint }> {
  if (!c.graduated) {
    const curve = await pump.fetchBondingCurve(c.mint);
    return { token: big(curve.virtualTokenReserves), quote: big(curve.virtualQuoteReserves) };
  }
  const [b, q] = await Promise.all([
    connection.getTokenAccountBalance(c.poolBase),
    connection.getTokenAccountBalance(c.poolQuote),
  ]);
  return { token: BigInt(b.value.amount), quote: BigInt(q.value.amount) };
}

/** Pre-instructions paid by the cranker so the vault doesn't spend on rent. */
async function vaultSetupIxs(c: Ctx): Promise<TransactionInstruction[]> {
  const ixs: TransactionInstruction[] = [
    createAssociatedTokenAccountIdempotentInstruction(
      payer.publicKey, c.vaultWsol, c.vault, NATIVE_MINT, TOKEN_PROGRAM_ID, ASSOCIATED_TOKEN_PROGRAM_ID,
    ),
    createAssociatedTokenAccountIdempotentInstruction(
      payer.publicKey, c.vaultToken, c.vault, c.mint, c.r.tokenProgram, ASSOCIATED_TOKEN_PROGRAM_ID,
    ),
  ];
  const volPda = c.graduated ? ammUserVolumePda(c.vault) : pumpUserVolumePda(c.vault);
  if (!(await connection.getAccountInfo(volPda))) {
    ixs.push(
      c.graduated
        ? await PUMP_AMM_SDK.initUserVolumeAccumulator({ payer: payer.publicKey, user: c.vault })
        : await PUMP_SDK.initUserVolumeAccumulator({ payer: payer.publicKey, user: c.vault }),
    );
  }
  return ixs;
}

// ------------------------------------------------------------------ steps
/** Routers that owe the protocol share can't route until it's verified. */
async function stepVerify(c: Ctx): Promise<boolean> {
  if (c.r.protocolVerified) return true;
  const ix = await program.methods
    .verifyProtocolShare()
    .accountsPartial({ router: c.router, sharingConfig: feeSharingConfigPda(c.mint) })
    .instruction();
  return (await send([ix], "verify-protocol", 100_000)) !== null;
}

async function stepClaim(c: Ctx) {
  const now = Math.floor(Date.now() / 1000);
  if (now < c.r.lastInflowTs.toNumber() + c.r.config.claimIntervalSecs.toNumber()) return;

  const inflows = await program.methods
    .accountInflows()
    .accountsPartial({
      cranker: payer.publicKey,
      global: globalPda(),
      router: c.router,
      vault: c.vault,
      vaultWsol: c.vaultWsol,
      systemProgram: SystemProgram.programId,
    })
    .instruction();

  const ixs: TransactionInstruction[] = [];
  const sharingAddress = feeSharingConfigPda(c.mint);
  const sharingInfo = await connection.getAccountInfo(sharingAddress);
  if (sharingInfo) {
    const sharingConfig = PUMP_SDK.decodeSharingConfig(sharingInfo);
    if (c.graduated) {
      ixs.push(
        await PUMP_SDK.transferCreatorFeesToPumpV2({
          payer: payer.publicKey,
          mint: c.mint,
          quoteMint: NATIVE_MINT,
          quoteTokenProgram: TOKEN_PROGRAM_ID,
        }),
      );
    }
    ixs.push(
      await PUMP_SDK.distributeCreatorFeesV2({
        mint: c.mint,
        sharingConfig,
        sharingConfigAddress: sharingAddress,
        quoteMint: NATIVE_MINT,
        payer: payer.publicKey,
        shouldInitializeAta: false,
        quoteTokenProgram: TOKEN_PROGRAM_ID,
      }),
    );
  }
  // pump rejects distributions below its minimum; fall back to accounting
  // whatever already sits in the vault.
  if (!(await send([...ixs, inflows], "claim"))) await send([inflows], "inflows-only");
}

async function stepObserve(c: Ctx) {
  const now = Math.floor(Date.now() / 1000);
  if (now < c.r.lastObserveTs.toNumber() + 300) return;
  const ix = await program.methods
    .observePrice(c.venue)
    .accountsPartial({ router: c.router, ...c.venueAccounts })
    .instruction();
  await send([ix], "observe", 100_000);
}

/** Build the pump instruction with the vault as the buyer. */
async function buildBuyIx(c: Ctx, lamports: bigint, slipBps: number): Promise<TransactionInstruction> {
  if (!c.graduated) {
    const [global, feeConfig, curve] = await Promise.all([
      pump.fetchGlobal(),
      pump.fetchFeeConfig(),
      pump.fetchBondingCurve(c.mint),
    ]);
    const quote = new BN(lamports.toString());
    const tokens = getBuyTokenAmountFromSolAmount({
      global,
      feeConfig,
      mintSupply: curve.tokenTotalSupply,
      bondingCurve: curve,
      amount: quote,
      quoteMint: NATIVE_MINT,
    });
    return PUMP_SDK.getBuyV2InstructionRaw({
      user: c.vault,
      mint: c.mint,
      creator: curve.creator,
      amount: tokens.muln(BPS - slipBps).divn(BPS),
      quoteAmount: quote, // max SOL cost; on-chain check is spent <= amount_in
      tokenProgram: c.r.tokenProgram,
      quoteMint: NATIVE_MINT,
      quoteTokenProgram: TOKEN_PROGRAM_ID,
    });
  }
  const state = await amm.swapSolanaState(c.pool, c.vault, c.vaultToken, c.vaultWsol);
  // SDK slippage is a percent and inflates max quote by it, so scale the
  // input down to keep max quote <= amount_in.
  const slipPct = slipBps / 100;
  const quote = new BN(((lamports * BigInt(BPS)) / BigInt(BPS + slipBps)).toString());
  const ixs = await PUMP_AMM_SDK.buyQuoteInput(state, quote, slipPct);
  return lastIxFor(ixs, PUMP_AMM_PROGRAM_ID);
}

async function buy(c: Ctx, purpose: number, bucket: bigint) {
  const slip: number = c.r.config.maxSlippageBps;
  const res = await reserves(c);
  const depthCap = (res.quote * BigInt(slip) * 9n) / (BigInt(BPS) * 10n); // 90% of on-chain cap
  let amount = bucket < depthCap ? bucket : depthCap;
  const free = BigInt(await connection.getBalance(c.vault)) - VAULT_RENT_RESERVE;
  if (!c.graduated && amount > free) amount = free; // curve buys spend native lamports
  if (amount < MIN_TRADE) return;

  if (!c.graduated && free < amount) {
    await send([await unwrapIx(c)], "unwrap");
  }
  const pumpIx = await buildBuyIx(c, amount, slip);
  const ix = await program.methods
    .executeBuy(purpose, c.venue, new BN(amount.toString()), pumpIx.data)
    .accountsPartial({
      cranker: payer.publicKey,
      global: globalPda(),
      router: c.router,
      vault: c.vault,
      mint: c.mint,
      vaultTokenAccount: c.vaultToken,
      vaultWsol: c.vaultWsol,
      ...c.venueAccounts,
      tokenProgram: c.r.tokenProgram,
      splTokenProgram: TOKEN_PROGRAM_ID,
      associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
      systemProgram: SystemProgram.programId,
    })
    .remainingAccounts(asRemaining(pumpIx, c.vault))
    .instruction();
  await send([...(await vaultSetupIxs(c)), ix], purpose === PURPOSE_BURN ? "buyback-burn" : "lp-buy", 800_000);
}

async function unwrapIx(c: Ctx) {
  return program.methods
    .unwrapWsol()
    .accountsPartial({ router: c.router, vault: c.vault, vaultWsol: c.vaultWsol, splTokenProgram: TOKEN_PROGRAM_ID })
    .instruction();
}

async function stepBurn(c: Ctx) {
  await buy(c, PURPOSE_BURN, big(c.r.burnBucket));
}

async function stepLiquidity(c: Ctx) {
  if (!c.graduated) return; // LP bucket waits for the pool
  await buy(c, PURPOSE_LP, big(c.r.lpBucket) / 2n);

  const r = await accounts.router.fetch(c.router);
  const inventory = big(r.lpTokenInventory);
  const quoteBucket = big(r.lpBucket);
  if (inventory === 0n || quoteBucket < MIN_TRADE) return;

  const slip: number = r.config.maxSlippageBps;
  const slipPct = slip / 100;
  const lpMint = lpMintPda(c.pool);
  const vaultLp = getAssociatedTokenAddressSync(lpMint, c.vault, true, TOKEN_2022_PROGRAM_ID);
  const state = await amm.liquiditySolanaState(c.pool, c.vault, c.vaultToken, c.vaultWsol, vaultLp);

  // Size the deposit by whichever side runs out first, with slippage headroom.
  const head = (v: bigint) => (v * BigInt(BPS)) / BigInt(BPS + slip);
  let lpToken: BN;
  let base: bigint;
  let quote: bigint;
  const fromBase = PUMP_AMM_SDK.depositAutocompleteQuoteAndLpTokenFromBase(state, new BN(head(inventory).toString()), slipPct);
  if (big(fromBase.quote) <= quoteBucket) {
    lpToken = fromBase.lpToken;
    base = head(inventory);
    quote = big(fromBase.quote);
  } else {
    const fromQuote = PUMP_AMM_SDK.depositAutocompleteBaseAndLpTokenFromQuote(state, new BN(head(quoteBucket).toString()), slipPct);
    lpToken = fromQuote.lpToken;
    base = big(fromQuote.base);
    quote = head(quoteBucket);
  }
  const cap = (v: bigint, max: bigint) => {
    const withSlip = (v * BigInt(BPS + slip)) / BigInt(BPS);
    return withSlip < max ? withSlip : max;
  };
  const tokenInMax = cap(base, inventory);
  const quoteInMax = cap(quote, quoteBucket);

  const depIx = lastIxFor(await PUMP_AMM_SDK.depositInstructions(state, lpToken, slipPct), PUMP_AMM_PROGRAM_ID);
  const ix = await program.methods
    .executeDeposit(new BN(tokenInMax.toString()), new BN(quoteInMax.toString()), depIx.data)
    .accountsPartial({
      cranker: payer.publicKey,
      global: globalPda(),
      router: c.router,
      vault: c.vault,
      mint: c.mint,
      vaultTokenAccount: c.vaultToken,
      vaultWsol: c.vaultWsol,
      pool: c.pool,
      poolBase: c.poolBase,
      poolQuote: c.poolQuote,
      lpMint,
      vaultLpAccount: vaultLp,
      tokenProgram: c.r.tokenProgram,
      lpTokenProgram: TOKEN_2022_PROGRAM_ID,
      splTokenProgram: TOKEN_PROGRAM_ID,
      associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
      systemProgram: SystemProgram.programId,
    })
    .remainingAccounts(asRemaining(depIx, c.vault))
    .instruction();
  await send([...(await vaultSetupIxs(c)), ix], "lp-deposit", 800_000);
}

// ------------------------------------------------------------------ holders
interface StoredEpoch {
  router: string;
  index: number;
  epoch: string;
  root: string;
  total: string;
  entries: { claimant: string; amount: string; proof: string[] }[];
}

const epochDir = (router: PublicKey) => join(DATA_DIR, "epochs", router.toBase58());

async function stepHolders(c: Ctx) {
  const now = Math.floor(Date.now() / 1000);
  const dir = epochDir(c.router);
  mkdirSync(dir, { recursive: true });

  // Post a new epoch once per claim interval when the bucket is big enough.
  const bucket = big(c.r.holdersBucket);
  const index = c.r.nextEpoch.toNumber();
  const lastFile = join(dir, `${index - 1}.json`);
  const lastPostedAt = index > 0 && existsSync(lastFile) ? Number(JSON.parse(readFileSync(lastFile, "utf8")).postedAt ?? 0) : 0;
  if (bucket >= MIN_EPOCH && now >= lastPostedAt + c.r.config.claimIntervalSecs.toNumber()) {
    const exclude = [
      c.vault,
      c.bc,
      c.pool,
      pumpPoolAuthorityPda(c.mint),
      feeSharingConfigPda(c.mint),
      ...(c.r.config.excludeCreator ? [c.r.authority as PublicKey] : []),
    ];
    const holders = await snapshotHolders(connection, c.mint, c.r.tokenProgram, {
      minBalance: big(c.r.config.minHolderBalance),
      exclude,
    });
    const entries = allocate(holders, bucket, MIN_PAYOUT);
    if (entries.length > 0) {
      const epoch = epochPda(c.router, index);
      const tree = buildTree(epoch, entries);
      const total = entries.reduce((a, e) => a + e.amount, 0n);
      const ix = await program.methods
        .postEpoch(new BN(index), [...tree.root], new BN(total.toString()), entries.length, new BN(CLAIM_WINDOW_SECS))
        .accountsPartial({ poster: payer.publicKey, global: globalPda(), router: c.router, epoch, systemProgram: SystemProgram.programId })
        .instruction();
      if (await send([ix], `post-epoch #${index} (${entries.length} holders)`)) {
        const stored: StoredEpoch & { postedAt: number } = {
          router: c.router.toBase58(),
          index,
          epoch: epoch.toBase58(),
          root: tree.root.toString("hex"),
          total: total.toString(),
          postedAt: now,
          entries: entries.map((e, i) => ({
            claimant: e.claimant.toBase58(),
            amount: e.amount.toString(),
            proof: proofFor(tree, i).map((p) => p.toString("hex")),
          })),
        };
        // Publish this file: holders and auditors can recompute the root from it.
        writeFileSync(join(dir, `${index}.json`), JSON.stringify(stored, null, 2));
      }
    }
  }

  // Push claims and close expired epochs.
  for (const f of readdirSync(dir).filter((n) => n.endsWith(".json"))) {
    const s: StoredEpoch = JSON.parse(readFileSync(join(dir, f), "utf8"));
    const epoch = new PublicKey(s.epoch);
    const e = await accounts.epoch.fetchNullable(epoch);
    if (!e) continue;
    if (now >= e.expiresAt.toNumber()) {
      const ix = await program.methods
        .closeEpoch()
        .accountsPartial({ router: c.router, epoch, poster: e.poster })
        .instruction();
      await send([ix], `close-epoch #${s.index}`, 100_000);
      continue;
    }
    if (!PUSH_CLAIMS || e.vetoed || now < e.claimableAt.toNumber()) continue;
    const bitmap: number[] = e.bitmap;
    const pending = s.entries
      .map((en, i) => ({ ...en, i }))
      .filter(({ i }) => (bitmap[i >> 3] & (1 << (i & 7))) === 0);
    for (let k = 0; k < pending.length; k += 2) {
      const ixs = await Promise.all(
        pending.slice(k, k + 2).map((en) =>
          program.methods
            .claim(en.i, new BN(en.amount), en.proof.map((p) => [...Buffer.from(p, "hex")]))
            .accountsPartial({
              payer: payer.publicKey,
              router: c.router,
              vault: c.vault,
              epoch,
              claimant: new PublicKey(en.claimant),
              systemProgram: SystemProgram.programId,
            })
            .instruction(),
        ),
      );
      await send(ixs, `claims #${s.index} ${k + ixs.length}/${pending.length}`, 200_000);
    }
  }
}

// ------------------------------------------------------------------ loop
async function tick() {
  const routers: { publicKey: PublicKey }[] = await accounts.router.all();
  for (const { publicKey } of routers) {
    try {
      let c = await loadCtx(publicKey);
      if (!(await stepVerify(c))) continue; // split doesn't pay the protocol share
      c = await loadCtx(publicKey);
      await stepClaim(c);
      await stepObserve(c);
      c = await loadCtx(publicKey);
      await stepBurn(c);
      await stepLiquidity(c);
      c = await loadCtx(publicKey);
      await stepHolders(c);
    } catch (e) {
      log(`router ${publicKey.toBase58()} error:`, (e as Error).message);
    }
  }
}

async function main() {
  log(`crank ${payer.publicKey.toBase58()} on ${RPC}`);
  for (;;) {
    await tick();
    await new Promise((r) => setTimeout(r, LOOP_SECS * 1000));
  }
}

main();
