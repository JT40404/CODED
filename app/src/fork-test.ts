/**
 * End-to-end test against a local mainnet fork (Surfpool).
 *
 *   surfpool start                       # terminal 1
 *   anchor deploy --provider.cluster localnet
 *   cd app
 *   RPC_URL=http://127.0.0.1:8899 ADMIN_KEYPAIR=$HOME/.config/solana/deployer.json npx tsx src/setup-global.ts
 *   npx tsx src/fork-test.ts             # this file
 *
 * It launches a "main" coin, points the 1% protocol fee at it, launches a
 * test coin with a mixed split, trades it from a fresh wallet, runs the
 * crank once, and prints PASS/FAIL for each step.
 */
import { homedir } from "node:os";
import { readFileSync } from "node:fs";

// Test-friendly crank settings; must be set before crank.ts loads.
process.env.RPC_URL ??= "http://127.0.0.1:8899";
process.env.CRANK_KEYPAIR ??= `${homedir()}/.config/solana/deployer.json`;
process.env.IDL_PATH ??= "../target/idl/coded_router.json";
process.env.MIN_TRADE_LAMPORTS ??= "100000"; // 0.0001 SOL
process.env.MIN_EPOCH_LAMPORTS ??= "1000000"; // 0.001 SOL
process.env.MIN_PAYOUT_LAMPORTS ??= "1000";
process.env.DATA_DIR ??= "./data-fork";

import BN from "bn.js";
import { AnchorProvider, Program, Wallet, type Idl } from "@coral-xyz/anchor";
import {
  ComputeBudgetProgram,
  Connection,
  Keypair,
  LAMPORTS_PER_SOL,
  PublicKey,
  SystemProgram,
  TransactionInstruction,
  TransactionMessage,
  VersionedTransaction,
} from "@solana/web3.js";
import { TOKEN_2022_PROGRAM_ID, getAssociatedTokenAddressSync } from "@solana/spl-token";
import {
  OnlinePumpSdk,
  PUMP_SDK,
  bondingCurvePda,
  feeSharingConfigPda,
  getBuyTokenAmountFromSolAmount,
  getSellSolAmountFromTokenAmount,
} from "@pump-fun/pump-sdk";

import { BPS, LP_MODE_BURN, VENUE_BONDING_CURVE, globalPda, protocolRouterPda, routerPda, vaultPda } from "./constants";
import { launchToken, type LaunchParams, type WalletLike } from "./launch";

const RPC = process.env.RPC_URL!;
const connection = new Connection(RPC, "confirmed");
const admin = Keypair.fromSecretKey(
  Uint8Array.from(JSON.parse(readFileSync(process.env.CRANK_KEYPAIR!, "utf8"))),
);
const idl = JSON.parse(readFileSync(process.env.IDL_PATH!, "utf8")) as Idl;
const program = new Program(idl, new AnchorProvider(connection, new Wallet(admin), { commitment: "confirmed" }));
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const accounts = program.account as any;
const pump = new OnlinePumpSdk(connection);

// ------------------------------------------------------------------ helpers
const results: { step: string; ok: boolean; detail: string }[] = [];
function check(step: string, ok: boolean, detail = "") {
  results.push({ step, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${step}${detail ? `  (${detail})` : ""}`);
}
const sol = (lamports: bigint | number | BN) => (Number(lamports.toString()) / LAMPORTS_PER_SOL).toFixed(6);

function keypairWallet(kp: Keypair): WalletLike {
  return {
    publicKey: kp.publicKey,
    async signAllTransactions(txs) {
      txs.forEach((t) => t.sign([kp]));
      return txs;
    },
  };
}

async function waitFor(sig: string) {
  for (let i = 0; i < 60; i++) {
    const { value } = await connection.getSignatureStatuses([sig]);
    if (value[0]?.err) throw new Error(`tx failed: ${JSON.stringify(value[0].err)}`);
    if (value[0]?.confirmationStatus === "confirmed" || value[0]?.confirmationStatus === "finalized") return;
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error(`tx not confirmed: ${sig}`);
}

async function sendAs(kp: Keypair, ixs: TransactionInstruction[]) {
  const { blockhash } = await connection.getLatestBlockhash();
  const tx = new VersionedTransaction(
    new TransactionMessage({
      payerKey: kp.publicKey,
      recentBlockhash: blockhash,
      instructions: [ComputeBudgetProgram.setComputeUnitLimit({ units: 400_000 }), ...ixs],
    }).compileToV0Message(),
  );
  tx.sign([kp]);
  const sig = await connection.sendTransaction(tx);
  await waitFor(sig);
}

const params = (name: string, symbol: string, split: LaunchParams["split"]): LaunchParams => ({
  name,
  symbol,
  description: "CODED fork test",
  image: new Blob(["test"]),
  devBuySol: 0,
  split,
  rules: { minHolderTokens: 0, maxSlippageBps: 1000, lpMode: "burn", claimIntervalSecs: 900, excludeCreator: true },
});
const fakeUpload = async () => "https://example.com/coded-fork-test.json";

async function trade(trader: Keypair, mint: PublicKey, rounds: number, solPerRound: number) {
  const [global, feeConfig] = await Promise.all([pump.fetchGlobal(), pump.fetchFeeConfig()]);
  const ata = getAssociatedTokenAddressSync(mint, trader.publicKey, false, TOKEN_2022_PROGRAM_ID);
  for (let i = 0; i < rounds; i++) {
    // Last round is small and held, so the trader ends up a holder without
    // leaving the price far above where the router's reference sits.
    const last = i === rounds - 1;
    const quote = new BN((last ? 0.5 : solPerRound) * LAMPORTS_PER_SOL);
    const buyState = await pump.fetchBuyState(mint, trader.publicKey, TOKEN_2022_PROGRAM_ID);
    const amount = getBuyTokenAmountFromSolAmount({
      global, feeConfig, mintSupply: buyState.bondingCurve.tokenTotalSupply,
      bondingCurve: buyState.bondingCurve, amount: quote, quoteMint: buyState.quoteMint,
    });
    await sendAs(trader, await PUMP_SDK.buyV2Instructions({
      global, bondingCurveAccountInfo: buyState.bondingCurveAccountInfo, bondingCurve: buyState.bondingCurve,
      associatedUserAccountInfo: buyState.associatedUserAccountInfo, mint, user: trader.publicKey,
      amount, quoteAmount: quote, slippage: 5, tokenProgram: TOKEN_2022_PROGRAM_ID, quoteTokenProgram: buyState.quoteTokenProgram,
    }));
    // Sell everything except on the last round, so the trader ends up a holder
    // and the price stays near where it started.
    if (i < rounds - 1) {
      const bal = new BN((await connection.getTokenAccountBalance(ata)).value.amount);
      const sellState = await pump.fetchSellState(mint, trader.publicKey, TOKEN_2022_PROGRAM_ID);
      const out = getSellSolAmountFromTokenAmount({
        global, feeConfig, mintSupply: sellState.bondingCurve.tokenTotalSupply,
        bondingCurve: sellState.bondingCurve, amount: bal,
      });
      await sendAs(trader, await PUMP_SDK.sellV2Instructions({
        global, bondingCurveAccountInfo: sellState.bondingCurveAccountInfo, bondingCurve: sellState.bondingCurve,
        mint, user: trader.publicKey, amount: bal, quoteAmount: out, slippage: 5, tokenProgram: TOKEN_2022_PROGRAM_ID,
        quoteTokenProgram: sellState.quoteTokenProgram,
      }));
    }
  }
}

// ------------------------------------------------------------------ test
async function main() {
  console.log(`admin ${admin.publicKey.toBase58()} on ${RPC}\n`);

  // 0. preconditions
  const g = await accounts.global.fetchNullable(globalPda());
  check("global exists", !!g, g ? "" : "run setup-global.ts first");
  if (!g) return summary();
  check("allowlist loaded", g.allowed.length > 0, `${g.allowed.length} entries`);

  const trader = Keypair.generate();
  await waitFor(await connection.requestAirdrop(trader.publicKey, 1_000 * LAMPORTS_PER_SOL));
  check("trader funded", true, trader.publicKey.toBase58());

  // 1. main coin
  const main = await launchToken({
    connection, program, wallet: keypairWallet(admin), uploadMetadata: fakeUpload,
    params: params("Coded Main", "CMAIN", { creatorBps: 5000, holdersBps: 0, lpBps: 0, burnBps: 5000, custom: [] }),
    onStatus: (m) => console.log(`  main: ${m}`),
  });
  check("launch main coin", true, main.mint.toBase58());

  // 2. protocol fee -> main coin
  const pRouter = protocolRouterPda(main.mint);
  const pVault = vaultPda(pRouter);
  const bc = bondingCurvePda(main.mint);
  await program.methods
    .initProtocolRouter(
      {
        holdersBps: 0, lpBps: 0, burnBps: BPS, minHolderBalance: new BN(0), maxSlippageBps: 1000,
        lpMode: LP_MODE_BURN, claimIntervalSecs: new BN(900), excludeCreator: true,
      },
      VENUE_BONDING_CURVE,
    )
    .accountsPartial({
      admin: admin.publicKey, global: globalPda(), mint: main.mint, router: pRouter, vault: pVault,
      venueA: bc, venueB: bc, venueC: bc, systemProgram: SystemProgram.programId,
    })
    .rpc();
  await program.methods.setProtocol(100)
    .accountsPartial({ admin: admin.publicKey, global: globalPda(), protocolRouter: pRouter })
    .rpc();
  const g2 = await accounts.global.fetch(globalPda());
  check("protocol fee set to 1%", g2.protocolVault.equals(pVault) && g2.protocolFeeBps === 100, pVault.toBase58());

  // 3. test coin with a mixed split
  const test = await launchToken({
    connection, program, wallet: keypairWallet(admin), uploadMetadata: fakeUpload,
    params: params("Coded Test", "CTEST", { creatorBps: 2000, holdersBps: 4000, lpBps: 2000, burnBps: 2000, custom: [] }),
    onStatus: (m) => console.log(`  test: ${m}`),
  });
  check("launch test coin", test.protocolFeeBps === 100, `${test.mint.toBase58()}, ${test.signatures.length} txs`);

  const tRouter = routerPda(test.mint, admin.publicKey);
  let r = await accounts.router.fetch(tRouter);
  check("router owes 1% and is verified", r.protocolFeeBps === 100 && r.protocolVerified);

  const scInfo = await connection.getAccountInfo(feeSharingConfigPda(test.mint));
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const sc: any = scInfo ? PUMP_SDK.decodeSharingConfig(scInfo) : null;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const holders: { address: PublicKey; shareBps: number }[] = sc?.shareholders ?? [];
  const pShare = holders.find((h) => h.address.equals(pVault))?.shareBps ?? 0;
  const total = holders.reduce((a, h) => a + Number(h.shareBps), 0);
  check("pump split locked with protocol share", !!sc?.adminRevoked && pShare === 100 && total === BPS,
    holders.map((h) => `${h.address.toBase58().slice(0, 4)}…=${h.shareBps}`).join(" "));

  // 4. trading volume
  console.log("\n  trading (20 rounds of 10 SOL)…");
  await trade(trader, test.mint, 20, 10);
  check("trades executed", true);

  // 5. crank: test coin first, then the protocol router
  const { processRouter } = await import("./crank");
  await processRouter(tRouter);
  r = await accounts.router.fetch(tRouter);
  check("fees claimed into router", Number(r.totalInflow) > 0, `${sol(r.totalInflow)} SOL`);
  check("BUYBACK AND BURN (vault as buyer)", Number(r.totalTokensBurned) > 0,
    `${r.totalTokensBurned.toString()} base units burned, burn bucket left ${sol(r.burnBucket)} SOL`);
  check("LP share held until graduation", Number(r.lpBucket) > 0, `${sol(r.lpBucket)} SOL waiting`);
  check("holder epoch posted", Number(r.nextEpoch) > 0, `next epoch index ${r.nextEpoch}`);

  await processRouter(pRouter);
  const pr = await accounts.router.fetch(pRouter);
  check("protocol router received its 1%", Number(pr.totalInflow) > 0, `${sol(pr.totalInflow)} SOL`);
  check("main coin bought back and burned", Number(pr.totalTokensBurned) > 0, `${pr.totalTokensBurned.toString()} base units`);

  summary();
  console.log(`
Holder claims open 1 hour after the epoch is posted. To test them, wait an
hour (or use Surfpool's time travel) and run:
  RPC_URL=${RPC} CRANK_KEYPAIR=${process.env.CRANK_KEYPAIR} IDL_PATH=${process.env.IDL_PATH} \\
  MIN_TRADE_LAMPORTS=100000 MIN_EPOCH_LAMPORTS=1000000 MIN_PAYOUT_LAMPORTS=1000 DATA_DIR=./data-fork \\
  npx tsx src/crank.ts
The trader wallet ${trader.publicKey.toBase58()} should receive SOL.`);
}

function summary() {
  const failed = results.filter((x) => !x.ok);
  console.log(`\n${results.length - failed.length}/${results.length} passed`);
  if (failed.length) console.log("Failed: " + failed.map((f) => f.step).join(", "));
}

main().catch((e) => {
  console.error("\nStopped with an error:\n", e?.logs ? e.logs.join("\n") : e);
  summary();
  process.exit(1);
});
