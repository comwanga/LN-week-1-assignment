use bitcoincore_rpc::{Auth, Client as BitcoinClient, RpcApi};
use reqwest::blocking::Client;
use serde_json::Value;

fn call_cln(method: &str, params: Value) -> Result<Value, Box<dyn std::error::Error>> {
    let rune = std::env::var("CLN_RUNE")?;
    let url = format!("http://localhost:3010/v1/{}", method);

    let client = Client::new();
    let response = client
        .post(&url)
        .json(&params)
        .header("Rune", rune)
        .send()?
        .json::<Value>()?;

    Ok(response)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Get blockchain info
    let rpc = BitcoinClient::new(
        "http://localhost:18443",
        Auth::UserPass("alice".to_string(), "password".to_string()),
    )?;

    println!("Blockchain Info: {:?}", rpc.get_blockchain_info()?);

    // Get Lightning node info
    let ln_info = call_cln("getinfo", serde_json::json!({}))?;
    println!("Lightning Node Info: {}", ln_info);

    // Create a new address for funding using lightning-cli and store it in CLN_ADDRESS

    // Check if wallet exists, if not Create a bitcoin wallet named 'mining_wallet' using bitcoin-cli for mining

    // Generate a new address and mine blocks to it. How many blocks need to mined? Why?

    // Fund the Lightning node by sending 0.1 BTC from the mining wallet to CLN_ADDRESS

    // Confirm the funding transaction by mining 6 blocks

    // Verify Lightning wallet balance using lightning-cli listfunds

    // Create an invoice with parameters and store the invoice string:
    // - Amount: 50,000 satoshis (50000000 millisatoshis)
    // - Label: Generate unique label using timestamp (e.g., "invoice_$(date +%s)")
    // - Description: "Coffee Payment"
    // - Expiry: 3600 seconds

    // Decode the invoice string using lightning-cli decodepay and verify the parameters
    // Output the invoice details in the specified format to out.txt
    // - Payment hash
    // - BOLT11 invoice string
    // - Amount
    // - Description
    // - Expiry time

    Ok(())
}
