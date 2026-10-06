/**
 * Run once, after you launch the protocol token:
 *   1. creates the protocol token's router, set to 100% buyback and burn
 *   2. points the protocol fee at it (default 1% of every new coin's fees)
 *
 *   RPC_URL=... ADMIN_KEYPAIR=... MAIN_MINT=<your token CA> PROTOCOL_FEE_BPS=100 \
 *     npx tsx src/setup-protocol.ts
 *
 * Coins launched before this runs owe nothing. Every coin launched after
 * owes the fee for life: pump.fun locks it into the coin's fee split, and
 * the CODED program won't route a coin's fees until it confirms that.
 */
import { readFileSync } from "node:fs";
import BN from "bn.js";
import { AnchorProvider, Program, Wallet, type Idl } from "@coral-xyz/anchor";
import { Connection, Keypair, PublicKey, SystemProgram } from "@solana/web3.js";
import { NATIVE_MINT, TOKEN_PROGRAM_ID, getAssociatedTokenAddressSync } from "@solana/spl-token";
import { OnlinePumpSdk, bondingCurvePda, canonicalPumpPoolPda } from "@pump-fun/pump-sdk";

import { BPS, LP_MODE_BURN, VENUE_AMM, VENUE_BONDING_CURVE, globalPda, protocolRouterPda, vaultPda } from "./constants";

const RPC = process.env.RPC_URL ?? "https://api.mainnet-beta.solana.com";
const IDL_PATH = process.env.IDL_PATH ?? "../target/idl/coded_router.json";
const FEE_BPS = Number(process.env.PROTOCOL_FEE_BPS ?? 100);

async function main() {
  const mint = new PublicKey(process.env.MAIN_MINT!);
  const connection = new Connection(RPC, "confirmed");
  const admin = Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(readFileSync(process.env.ADMIN_KEYPAIR!, "utf8"))),
  );
  const idl = JSON.parse(readFileSync(IDL_PATH, "utf8")) as Idl;
  const program = new Program(idl, new AnchorProvider(connection, new Wallet(admin), {}));

  const mintInfo = await connection.getAccountInfo(mint);
  if (!mintInfo) throw new Error("MAIN_MINT not found on this cluster.");
  const tokenProgram = mintInfo.owner;

  const router = protocolRouterPda(mint);
  const vault = vaultPda(router);

  if (!(await connection.getAccountInfo(router))) {
    const curve = await new OnlinePumpSdk(connection).fetchBondingCurve(mint);
    const pool = canonicalPumpPoolPda(mint);
    const venue = curve.complete
      ? {
          v: VENUE_AMM,
          a: pool,
          b: getAssociatedTokenAddressSync(mint, pool, true, tokenProgram),
          c: getAssociatedTokenAddressSync(NATIVE_MINT, pool, true, TOKEN_PROGRAM_ID),
        }
      : { v: VENUE_BONDING_CURVE, a: bondingCurvePda(mint), b: bondingCurvePda(mint), c: bondingCurvePda(mint) };

    await program.methods
      .initProtocolRouter(
        {
          holdersBps: 0,
          lpBps: 0,
          burnBps: BPS,
          minHolderBalance: new BN(0),
          maxSlippageBps: 300,
          lpMode: LP_MODE_BURN,
          claimIntervalSecs: new BN(3600),
          excludeCreator: true,
        },
        venue.v,
      )
      .accountsPartial({
        admin: admin.publicKey,
        global: globalPda(),
        mint,
        router,
        vault,
        venueA: venue.a,
        venueB: venue.b,
        venueC: venue.c,
        systemProgram: SystemProgram.programId,
      })
      .rpc();
    console.log("protocol router created", router.toBase58());
  }

  await program.methods
    .setProtocol(FEE_BPS)
    .accountsPartial({ admin: admin.publicKey, global: globalPda(), protocolRouter: router })
    .rpc();
  console.log(`protocol fee set: ${FEE_BPS / 100}% of every new coin's creator fees`);
  console.log(`buyback vault (added to each new coin's pump.fun split): ${vault.toBase58()}`);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
