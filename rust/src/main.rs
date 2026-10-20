use serde_json::Value;
use std::{
    error::Error,
    fs,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const WALLET: &str = "mining_wallet";
const AMOUNT_MSAT: u64 = 50_000_000;
const DESCRIPTION: &str = "Coffee Payment";
const EXPIRY: u64 = 3600;

// Invoke the supplied nodes without a shell, preserving spaces in RPC arguments.
fn docker_cli(container: &str, executable: &str, args: &[&str]) -> Result<Value> {
    let output = Command::new("docker")
        .args(["exec", container, executable])
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "{executable} failed ({}): {} {}",
            output.status,
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let stdout = String::from_utf8(output.stdout)?;
    // bitcoin-cli returns addresses and transaction IDs as unquoted strings.
    Ok(serde_json::from_str(stdout.trim())
        .unwrap_or_else(|_| Value::String(stdout.trim().to_owned())))
}

fn bitcoin_cli(args: &[&str]) -> Result<Value> {
    let mut options = vec!["-regtest", "-rpcuser=alice", "-rpcpassword=password"];
    options.extend_from_slice(args);
    docker_cli("bitcoind", "bitcoin-cli", &options)
}

fn mining_cli(args: &[&str]) -> Result<Value> {
    let wallet_option = format!("-rpcwallet={WALLET}");
    let mut options = vec![wallet_option.as_str()];
    options.extend_from_slice(args);
    bitcoin_cli(&options)
}

fn call_cln(args: &[&str]) -> Result<Value> {
    let mut options = vec!["--network=regtest"];
    options.extend_from_slice(args);
    docker_cli("ln-node", "lightning-cli", &options)
}

fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn string(value: &Value) -> Result<&str> {
    value
        .as_str()
        .ok_or_else(|| "Expected an RPC string".into())
}

// Both startup and Lightning's Bitcoin polling take time; wait for actual state.
fn wait_for(label: &str, mut check: impl FnMut() -> Result<bool>) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut last_error = String::new();
    while Instant::now() < deadline {
        match check() {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) => last_error = error.to_string(),
        }
        thread::sleep(Duration::from_secs(1));
    }
    Err(format!("Timed out waiting for {label}. {last_error}").into())
}

fn main() -> Result<()> {
    wait_for("Bitcoin RPC", || {
        bitcoin_cli(&["getblockchaininfo"]).map(|_| true)
    })?;
    wait_for("Lightning RPC", || call_cln(&["getinfo"]).map(|_| true))?;

    // Generate a deposit address owned by the Lightning wallet.
    let address_result = call_cln(&["newaddr"])?;
    let lightning_address = string(&address_result["bech32"])?;

    // Reuse a loaded mining wallet, load it from disk, or create it on first use.
    let loaded_wallets = bitcoin_cli(&["listwallets"])?;
    let loaded = loaded_wallets
        .as_array()
        .ok_or("Expected a wallet list")?
        .iter()
        .any(|wallet| wallet.as_str() == Some(WALLET));
    if !loaded {
        let directory = bitcoin_cli(&["listwalletdir"])?;
        let exists = directory["wallets"]
            .as_array()
            .ok_or("Expected a wallet directory")?
            .iter()
            .any(|wallet| wallet["name"].as_str() == Some(WALLET));
        bitcoin_cli(&[if exists { "loadwallet" } else { "createwallet" }, WALLET])?;
    }
    let address_result = mining_cli(&["getnewaddress"])?;
    let mining_address = string(&address_result)?;

    // Coinbase rewards need 100 additional blocks before they are spendable.
    // Starting from an empty chain, the balance becomes positive at block 101.
    // Keep enough for the 0.1 BTC deposit and its transaction fee on repeat runs.
    let mut blocks_mined = 0;
    let balance = loop {
        let balance = mining_cli(&["getbalance"])?
            .as_f64()
            .ok_or("Expected a numeric Bitcoin balance")?;
        println!("After mining {blocks_mined} blocks, spendable balance: {balance} BTC");
        if balance >= 0.1001 {
            break balance;
        }
        require(
            blocks_mined < 101,
            "Mining did not produce a spendable balance",
        )?;
        // Batch the initial immature rewards to avoid 100 Docker round trips.
        let count = if blocks_mined == 0 { 100 } else { 1 };
        bitcoin_cli(&["generatetoaddress", &count.to_string(), mining_address])?;
        blocks_mined += count;
    };
    println!("Mined {blocks_mined} blocks; spendable balance: {balance} BTC");

    // Fund Lightning with 0.1 BTC and give that deposit six confirmations.
    let transaction = mining_cli(&["sendtoaddress", lightning_address, "0.1"])?;
    let funding_txid = string(&transaction)?;
    bitcoin_cli(&["generatetoaddress", "6", mining_address])?;
    wait_for("Lightning to recognize the confirmed deposit", || {
        let funds = call_cln(&["listfunds"])?;
        let outputs = funds["outputs"]
            .as_array()
            .ok_or("Expected wallet outputs")?;
        Ok(outputs.iter().any(|output| {
            output["txid"].as_str() == Some(funding_txid)
                && output["status"].as_str() == Some("confirmed")
                && output["amount_msat"].as_u64() == Some(10_000_000_000)
        }))
    })?;
    println!("Lightning funding confirmed: {funding_txid} (0.1 BTC)");

    // Use a unique label so rerunning the exercise creates another valid invoice.
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let label = format!("coffee_{timestamp}_{}", std::process::id());
    let invoice = call_cln(&[
        "invoice",
        &AMOUNT_MSAT.to_string(),
        &label,
        DESCRIPTION,
        &EXPIRY.to_string(),
    ])?;

    // Decode the real BOLT11 invoice and verify every value before reporting it.
    let bolt11 = string(&invoice["bolt11"])?;
    let payment_hash = string(&invoice["payment_hash"])?;
    // decode also works on recent CLN versions, which removed decodepay.
    let decoded = call_cln(&["decode", bolt11])?;
    require(
        payment_hash.len() == 64
            && payment_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "Invalid payment hash",
    )?;
    require(
        bolt11.starts_with("lnbcrt") && bolt11.len() > 100,
        "Expected a regtest BOLT11 invoice",
    )?;
    require(
        decoded["payment_hash"] == payment_hash,
        "Payment hash mismatch",
    )?;
    require(
        decoded["amount_msat"].as_u64() == Some(AMOUNT_MSAT),
        "Amount mismatch",
    )?;
    require(
        decoded["description"] == DESCRIPTION,
        "Description mismatch",
    )?;
    require(
        decoded["expiry"].as_u64() == Some(EXPIRY),
        "Expiry mismatch",
    )?;

    // Always write out.txt in the repository root, even when launched from rust/.
    let output_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../out.txt");
    fs::write(
        &output_path,
        format!("{payment_hash}\n{bolt11}\n{AMOUNT_MSAT}\n{DESCRIPTION}\n{EXPIRY}\n"),
    )?;
    println!("Invoice details written to {}", output_path.display());
    Ok(())
}
