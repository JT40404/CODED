# CODED

Programmable routing for pump.fun creator fees: holder payouts, auto-liquidity,
buyback and burn, plus fixed creator and custom wallets.

```
public/index.html        The website (Vercel serves this folder)
api/rpc.ts               Vercel function: RPC proxy, keeps your RPC key server-side
api/metadata.ts          Vercel function: pins coin image + metadata to IPFS
idl/                     Commit target/idl/coded_router.json here after anchor build
app/                     TypeScript: launcher bundle, crank, setup, merkle
programs/coded_router/   Anchor program (Rust)
vercel.json              Build settings
```

## Deploy the site (GitHub + Vercel)

1. Push this folder to a new GitHub repo.
2. In Vercel: **Add New → Project → Import** the repo. Leave Framework as
   "Other"; `vercel.json` supplies the install, build and output settings.
3. Add environment variables (see `.env.example`):
   `RPC_URL` (a paid RPC such as Helius or QuickNode), `PINATA_JWT`,
   `CODED_PROGRAM_ID`.
4. Deploy. The site works right away. Launching stays off until
   `idl/coded_router.json` exists, which happens after you deploy the program.

When the program is deployed: copy `target/idl/coded_router.json` into `idl/`,
set `CODED_PROGRAM_ID` in Vercel, commit and push. Vercel rebuilds and the
Launch page goes live.

## How it fits together

1. **Launch** (`app/src/launch.ts`, one wallet approval, three transactions)
   1. pump.fun `create_v2` (+ optional first buy)
   2. CODED `initialize_router` + pump.fun `create_fee_sharing_config`
   3. pump.fun `update_fee_shares_v2` — sets every recipient and revokes the
      admin. **The split between CODED's vault, the creator and custom wallets
      is permanent from here.**
2. **Crank** (`app/src/crank.ts`, permissionless, anyone can run it)
   - claim: pump `transfer_creator_fees_to_pump_v2` + `distribute_creator_fees_v2`, then CODED `account_inflows`
   - `observe_price` (rate limited) keeps a slow reference price
   - `execute_buy` (burn): vault buys on the curve or PumpSwap, tokens are burned
   - `execute_buy` (LP) + `execute_deposit`: adds liquidity, burns or locks LP
   - holders: snapshot → `post_epoch` (merkle root) → `claim` pushes → `close_epoch`
3. **Config changes**: `propose_config` → wait 48h → `apply_config`.
   `set_authority(default)` renounces and freezes the ratios.

## Protocol fee (CODED token buyback)

1% of the creator fees of every coin launched through CODED buys back and
burns your main token.

- After you launch the main token, run once:
  `RPC_URL=... ADMIN_KEYPAIR=... MAIN_MINT=<CA> PROTOCOL_FEE_BPS=100 npx tsx app/src/setup-protocol.ts`
  This creates a dedicated router for the main token (100% buyback and burn)
  and stores its mint, router, vault and the fee in Global.
- The launcher reads Global, scales the creator's split to 99% and adds the
  protocol vault at 1% to the pump.fun split, which pump.fun then locks.
- Each router snapshots the fee and vault when it's created and can't run
  `account_inflows` until `verify_protocol_share` confirms the locked pump.fun
  split pays that vault at least that much. The launcher sends the
  verification as its last transaction; the crank retries it.
- The crank treats the protocol router like any other: it claims, then buys
  and burns the main token every cycle.
- Coins launched before `setup-protocol` owe nothing. `set_protocol` with a
  new fee only affects coins launched afterwards (hard cap 5%).

## Safety model

The vault PDA signs exactly one pump.fun instruction per call, and only if
its (program, discriminator) is on the admin allowlist. Every venue account is
re-derived from seeds. Before the CPI: trade size ≤ pool depth × slippage, spot
within slippage of the reference price. After the CPI: SOL spent ≤ approved
amount, tokens received ≥ quoted minimum (LP ≥ fair share for deposits).
Pausing blocks new activity but never claims, closes or LP release.

Raw account reads (bonding curve reserves, token amounts, mint supply) are
in `constants.rs`. Re-check them against pump's IDL before every deploy.

## Build and deploy

```bash
# program
anchor keys sync && anchor build
anchor deploy --provider.cluster mainnet
# hand the upgrade authority to a multisig (Squads) straight away

# client
cd app && npm install
npm run typecheck
npm run test:merkle             # must match the Rust test vector
ADMIN_KEYPAIR=... ROOT_POSTER=<indexer pubkey> npx tsx src/setup-global.ts
RPC_URL=... CRANK_KEYPAIR=... npx tsx src/crank.ts

# website bundle locally (Vercel runs this for you)
npm run build:web --prefix app
```

The crank and setup scripts run on your own server, not on Vercel: they need a
keypair and run continuously.

Publish `app/data/epochs/**.json` so anyone can recompute payout roots.

## Test before mainnet

- Run against a mainnet fork (`solana-test-validator --clone` the pump programs
  and accounts, or Surfpool). **Confirm pump.fun accepts a PDA as the buyer**
  in `buy_v2` and PumpSwap `buy`/`deposit` — this is the main untested assumption.
- Full flow: launch → trades → claim → burn → graduate → LP → epoch → claims → close.
- Get an independent audit.

## Known limits

- SOL-paired coins only (USDC-paired coins are not handled).
- Holder snapshots are point-in-time; consider randomised timing or
  time-weighted balances to blunt snapshot gaming.
- pump.fun admins retain platform powers (e.g. `admin_cto`) CODED can't override.
- Regular payouts to holders may have legal implications; get advice.
