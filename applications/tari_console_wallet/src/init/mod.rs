// Copyright 2020. The Tari Project
//
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
// following conditions are met:
//
// 1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
// disclaimer.
//
// 2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
// following disclaimer in the documentation and/or other materials provided with the distribution.
//
// 3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote
// products derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
// INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
// DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
// SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
// SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
// WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
// USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use std::{fs, path::PathBuf, str::FromStr, sync::Arc};

use log::*;
use rpassword::prompt_password_stdout;
use rustyline::Editor;
use tari_app_utilities::identity_management::setup_node_identity;
use tari_common::{
    configuration::bootstrap::prompt,
    exit_codes::{ExitCode, ExitError},
};
use tari_comms::{
    multiaddr::Multiaddr,
    peer_manager::{Peer, PeerFeatures},
    types::CommsPublicKey,
    NodeIdentity,
};
use tari_core::transactions::CryptoFactories;
use tari_crypto::keys::PublicKey;
use tari_key_manager::{cipher_seed::CipherSeed, mnemonic::MnemonicLanguage};
use tari_p2p::{peer_seeds::SeedPeer, TransportType};
use tari_shutdown::ShutdownSignal;
use tari_utilities::{ByteArray, SafePassword};
use tari_wallet::{
    error::{WalletError, WalletStorageError},
    output_manager_service::storage::database::OutputManagerDatabase,
    storage::{
        database::{WalletBackend, WalletDatabase},
        sqlite_utilities::initialize_sqlite_database_backends,
    },
    wallet::{derive_comms_secret_key, read_or_create_master_seed},
    Wallet,
    WalletConfig,
    WalletSqlite,
};

use crate::{
    cli::Cli,
    utils::db::{get_custom_base_node_peer_from_db, set_custom_base_node_peer_in_db},
    wallet_modes::{PeerConfig, WalletMode},
    ApplicationConfig,
};

pub const LOG_TARGET: &str = "wallet::console_wallet::init";
const TARI_WALLET_PASSWORD: &str = "TARI_WALLET_PASSWORD";

#[derive(Clone, Copy)]
pub enum WalletBoot {
    New,
    Existing,
    Recovery,
}

/// Gets the password provided by command line argument or environment variable if available.
/// Otherwise prompts for the password to be typed in.
pub fn get_or_prompt_password(
    arg_password: Option<SafePassword>,
    config_password: Option<SafePassword>,
) -> Result<Option<SafePassword>, ExitError> {
    if arg_password.is_some() {
        return Ok(arg_password);
    }

    let env = std::env::var_os(TARI_WALLET_PASSWORD);
    if let Some(p) = env {
        let env_password = p
            .into_string()
            .map_err(|_| ExitError::new(ExitCode::IOError, "Failed to convert OsString into String"))?;
        return Ok(Some(env_password.into()));
    }

    if config_password.is_some() {
        return Ok(config_password);
    }

    let password = prompt_password("Wallet password: ")?;

    Ok(Some(password))
}

fn prompt_password(prompt: &str) -> Result<SafePassword, ExitError> {
    let password = loop {
        let pass = prompt_password_stdout(prompt).map_err(|e| ExitError::new(ExitCode::IOError, e))?;
        if pass.is_empty() {
            println!("Password cannot be empty!");
            continue;
        } else {
            break pass;
        }
    };

    Ok(SafePassword::from(password))
}

/// Allows the user to change the password of the wallet.
pub async fn change_password(
    config: &ApplicationConfig,
    arg_password: Option<SafePassword>,
    shutdown_signal: ShutdownSignal,
    non_interactive_mode: bool,
) -> Result<(), ExitError> {
    let mut wallet = init_wallet(config, arg_password, None, None, shutdown_signal, non_interactive_mode).await?;

    let passphrase = prompt_password("New wallet password: ")?;
    let confirmed = prompt_password("Confirm new password: ")?;

    if passphrase != confirmed {
        return Err(ExitError::new(ExitCode::InputError, "Passwords don't match!"));
    }

    wallet
        .remove_encryption()
        .await
        .map_err(|e| ExitError::new(ExitCode::WalletError, e))?;

    wallet
        .apply_encryption(passphrase)
        .await
        .map_err(|e| ExitError::new(ExitCode::WalletError, e))?;

    println!("Wallet password changed successfully.");

    Ok(())
}

/// Populates the PeerConfig struct from:
/// 1. The custom peer in the wallet config if it exists
/// 2. The custom peer in the wallet db if it exists
/// 3. The detected local base node if any
/// 4. The service peers defined in config they exist
/// 5. The peer seeds defined in config
pub async fn get_base_node_peer_config(
    config: &ApplicationConfig,
    wallet: &mut WalletSqlite,
    non_interactive_mode: bool,
) -> Result<PeerConfig, ExitError> {
    let mut selected_base_node = match config.wallet.custom_base_node {
        Some(ref custom) => SeedPeer::from_str(custom)
            .map(|node| Some(Peer::from(node)))
            .map_err(|err| ExitError::new(ExitCode::ConfigError, &format!("Malformed custom base node: {}", err)))?,
        None => get_custom_base_node_peer_from_db(wallet),
    };

    // If the user has not explicitly set a base node in the config, we try detect one
    if !non_interactive_mode && config.wallet.custom_base_node.is_none() {
        if let Some(detected_node) = detect_local_base_node().await {
            match selected_base_node {
                Some(ref base_node) if base_node.public_key == detected_node.public_key => {
                    // Skip asking because it's already set
                },
                Some(_) | None => {
                    println!(
                        "Local Base Node detected with public key {} and address {}",
                        detected_node.public_key,
                        detected_node.addresses.first().unwrap()
                    );
                    if prompt(
                        "Would you like to use this base node? IF YOU DID NOT START THIS BASE NODE YOU SHOULD SELECT \
                         NO (Y/n)",
                    ) {
                        let address = detected_node.addresses.first().ok_or_else(|| {
                            ExitError::new(ExitCode::ConfigError, "No address found for detected base node")
                        })?;
                        set_custom_base_node_peer_in_db(wallet, &detected_node.public_key, address)?;
                        selected_base_node = Some(detected_node.into());
                    }
                },
            }
        }
    }

    // config
    let base_node_peers = config
        .wallet
        .base_node_service_peers
        .iter()
        .map(|s| SeedPeer::from_str(s))
        .map(|r| r.map(Peer::from))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| ExitError::new(ExitCode::ConfigError, format!("Malformed base node peer: {}", err)))?;

    // peer seeds
    let peer_seeds = config
        .peer_seeds
        .peer_seeds
        .iter()
        .map(|s| SeedPeer::from_str(s))
        .map(|r| r.map(Peer::from))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| ExitError::new(ExitCode::ConfigError, format!("Malformed seed peer: {}", err)))?;

    let peer_config = PeerConfig::new(selected_base_node, base_node_peers, peer_seeds);
    debug!(target: LOG_TARGET, "base node peer config: {:?}", peer_config);

    Ok(peer_config)
}

/// Determines which mode the wallet should run in.
pub(crate) fn wallet_mode(cli: &Cli, boot_mode: WalletBoot) -> WalletMode {
    // Recovery mode
    if matches!(boot_mode, WalletBoot::Recovery) {
        if cli.non_interactive_mode {
            return WalletMode::RecoveryDaemon;
        } else {
            return WalletMode::RecoveryTui;
        }
    }

    match (cli.non_interactive_mode, cli.input_file.clone(), cli.command2.clone()) {
        // TUI mode
        (false, None, None) => WalletMode::Tui,
        // GRPC mode
        (true, None, None) => WalletMode::Grpc,
        // Script mode
        (_, Some(path), None) => WalletMode::Script(path),
        // Command mode
        (_, None, Some(command)) => WalletMode::Command(Box::new(command)), // WalletMode::Command(command),
        // Invalid combinations
        _ => WalletMode::Invalid,
    }
}

/// Set up the app environment and state for use by the UI
#[allow(clippy::too_many_lines)]
pub async fn init_wallet(
    config: &ApplicationConfig,
    arg_password: Option<SafePassword>,
    seed_words_file_name: Option<PathBuf>,
    recovery_seed: Option<CipherSeed>,
    shutdown_signal: ShutdownSignal,
    non_interactive_mode: bool,
) -> Result<WalletSqlite, ExitError> {
    fs::create_dir_all(
        &config
            .wallet
            .db_file
            .parent()
            .expect("console_wallet_db_file cannot be set to a root directory"),
    )
    .map_err(|e| ExitError::new(ExitCode::WalletError, format!("Error creating Wallet folder. {}", e)))?;
    fs::create_dir_all(&config.wallet.p2p.datastore_path)
        .map_err(|e| ExitError::new(ExitCode::WalletError, format!("Error creating peer db folder. {}", e)))?;

    debug!(target: LOG_TARGET, "Running Wallet database migrations");

    // test encryption by initializing with no passphrase...
    let db_path = &config.wallet.db_file;

    let result = initialize_sqlite_database_backends(db_path, None, config.wallet.db_connection_pool_size);
    let (backends, wallet_encrypted) = match result {
        Ok(backends) => {
            // wallet is not encrypted
            (backends, false)
        },
        Err(WalletStorageError::NoPasswordError) => {
            // get supplied or prompt password
            let passphrase = get_or_prompt_password(arg_password.clone(), config.wallet.password.clone())?;
            let backends =
                initialize_sqlite_database_backends(db_path, passphrase, config.wallet.db_connection_pool_size)?;
            (backends, true)
        },
        Err(e) => {
            return Err(e.into());
        },
    };
    let (wallet_backend, transaction_backend, output_manager_backend, contacts_backend, key_manager_backend) = backends;
    let wallet_db = WalletDatabase::new(wallet_backend);
    let output_db = OutputManagerDatabase::new(output_manager_backend.clone());

    debug!(
        target: LOG_TARGET,
        "Databases Initialized. Wallet encrypted? {}.", wallet_encrypted
    );

    let node_address = match config.wallet.p2p.public_address.clone() {
        Some(addr) => addr,
        None => match wallet_db.get_node_address()? {
            Some(addr) => addr,
            None => Multiaddr::empty(),
        },
    };

    let master_seed = read_or_create_master_seed(recovery_seed.clone(), &wallet_db)?;

    let node_identity = match config.wallet.identity_file.as_ref() {
        Some(identity_file) => {
            warn!(
                target: LOG_TARGET,
                "Node identity overridden by file {}",
                identity_file.to_string_lossy()
            );
            setup_node_identity(
                identity_file,
                Some(&node_address),
                true,
                PeerFeatures::COMMUNICATION_CLIENT,
            )?
        },
        None => setup_identity_from_db(&wallet_db, &master_seed, node_address.clone())?,
    };

    let mut wallet_config = config.wallet.clone();
    if let TransportType::Tor = config.wallet.p2p.transport.transport_type {
        wallet_config.p2p.transport.tor.identity = wallet_db.get_tor_id()?;
    }

    let factories = CryptoFactories::default();

    let mut wallet = Wallet::start(
        config.wallet.clone(),
        config.peer_seeds.clone(),
        config.auto_update.clone(),
        node_identity,
        factories,
        wallet_db,
        output_db,
        transaction_backend,
        output_manager_backend,
        contacts_backend,
        key_manager_backend,
        shutdown_signal,
        master_seed,
    )
    .await
    .map_err(|e| match e {
        WalletError::CommsInitializationError(cie) => cie.to_exit_error(),
        e => ExitError::new(
            ExitCode::WalletError,
            &format!("Error creating Wallet Container: {}", e),
        ),
    })?;
    if let Some(hs) = wallet.comms.hidden_service() {
        wallet
            .db
            .set_tor_identity(hs.tor_identity().clone())
            .map_err(|e| ExitError::new(ExitCode::WalletError, format!("Problem writing tor identity. {}", e)))?;
    }

    if !wallet_encrypted {
        debug!(target: LOG_TARGET, "Wallet is not encrypted.");

        // create using --password arg if supplied and skip seed words confirmation
        let passphrase = match arg_password {
            Some(password) => {
                debug!(target: LOG_TARGET, "Setting password from command line argument.");
                password
            },
            None => {
                debug!(target: LOG_TARGET, "Prompting for password.");
                let password = prompt_password("Create wallet password: ")?;
                let confirmed = prompt_password("Confirm wallet password: ")?;

                if password != confirmed {
                    return Err(ExitError::new(ExitCode::InputError, "Passwords don't match!"));
                }

                password
            },
        };

        wallet.apply_encryption(passphrase).await?;

        debug!(target: LOG_TARGET, "Wallet encrypted.");

        if !non_interactive_mode && recovery_seed.is_none() {
            match confirm_seed_words(&mut wallet) {
                Ok(()) => {
                    print!("\x1Bc"); // Clear the screen
                },
                Err(error) => {
                    return Err(error);
                },
            };
        }
    }
    if let Some(file_name) = seed_words_file_name {
        let seed_words = wallet.get_seed_words(&MnemonicLanguage::English)?.join(" ");
        let _result = fs::write(file_name, seed_words).map_err(|e| {
            ExitError::new(
                ExitCode::WalletError,
                &format!("Problem writing seed words to file: {}", e),
            )
        });
    };

    Ok(wallet)
}

async fn detect_local_base_node() -> Option<SeedPeer> {
    use tari_app_grpc::tari_rpc::{base_node_client::BaseNodeClient, Empty};
    const COMMON_BASE_NODE_GRPC_ADDRESS: &str = "http://127.0.0.1:18142";

    let mut node_conn = BaseNodeClient::connect(COMMON_BASE_NODE_GRPC_ADDRESS).await.ok()?;
    let resp = node_conn.identify(Empty {}).await.ok()?;
    let identity = resp.get_ref();
    let public_key = CommsPublicKey::from_bytes(&identity.public_key).ok()?;
    let address = Multiaddr::from_str(&identity.public_address).ok()?;
    Some(SeedPeer::new(public_key, vec![address]))
}

fn setup_identity_from_db<D: WalletBackend + 'static>(
    wallet_db: &WalletDatabase<D>,
    master_seed: &CipherSeed,
    node_address: Multiaddr,
) -> Result<Arc<NodeIdentity>, ExitError> {
    let node_features = wallet_db
        .get_node_features()?
        .unwrap_or(PeerFeatures::COMMUNICATION_CLIENT);

    let identity_sig = wallet_db.get_comms_identity_signature()?;

    let comms_secret_key = derive_comms_secret_key(master_seed)?;

    // This checks if anything has changed by validating the previous signature and if invalid, setting identity_sig
    // to None
    let identity_sig = identity_sig.filter(|sig| {
        let comms_public_key = CommsPublicKey::from_secret_key(&comms_secret_key);
        sig.is_valid(&comms_public_key, node_features, [&node_address])
    });

    // SAFETY: we are manually checking the validity of this signature before adding Some(..)
    let node_identity = Arc::new(NodeIdentity::with_signature_unchecked(
        comms_secret_key,
        node_address,
        node_features,
        identity_sig,
    ));
    if !node_identity.is_signed() {
        node_identity.sign();
        // unreachable panic: signed above
        let sig = node_identity
            .identity_signature_read()
            .as_ref()
            .expect("unreachable panic")
            .clone();
        wallet_db.set_comms_identity_signature(sig)?;
    }

    Ok(node_identity)
}

/// Starts the wallet by setting the base node peer, and restarting the transaction and broadcast protocols.
pub async fn start_wallet(
    wallet: &mut WalletSqlite,
    base_node: &Peer,
    wallet_mode: &WalletMode,
) -> Result<(), ExitError> {
    // TODO gRPC interfaces for setting base node #LOGGED
    debug!(target: LOG_TARGET, "Setting base node peer");

    let net_address = base_node
        .addresses
        .first()
        .ok_or_else(|| ExitError::new(ExitCode::ConfigError, "Configured base node has no address!"))?;

    wallet
        .set_base_node_peer(base_node.public_key.clone(), net_address.address.clone())
        .await
        .map_err(|e| {
            ExitError::new(
                ExitCode::WalletError,
                &format!("Error setting wallet base node peer. {}", e),
            )
        })?;

    // Restart transaction protocols if not running in script or command modes

    if !matches!(wallet_mode, WalletMode::Command(_)) && !matches!(wallet_mode, WalletMode::Script(_)) {
        if let Err(e) = wallet.transaction_service.restart_transaction_protocols().await {
            error!(target: LOG_TARGET, "Problem restarting transaction protocols: {}", e);
        }
        if let Err(e) = wallet.transaction_service.validate_transactions().await {
            error!(
                target: LOG_TARGET,
                "Problem validating and restarting transaction protocols: {}", e
            );
        }

        // validate transaction outputs
        validate_txos(wallet).await?;
    }
    Ok(())
}

async fn validate_txos(wallet: &mut WalletSqlite) -> Result<(), ExitError> {
    debug!(target: LOG_TARGET, "Starting TXO validations.");

    wallet.output_manager_service.validate_txos().await.map_err(|e| {
        error!(target: LOG_TARGET, "Error validating Unspent TXOs: {}", e);
        ExitError::new(ExitCode::WalletError, e)
    })?;

    debug!(target: LOG_TARGET, "TXO validations started.");

    Ok(())
}

fn confirm_seed_words(wallet: &mut WalletSqlite) -> Result<(), ExitError> {
    let seed_words = wallet.get_seed_words(&MnemonicLanguage::English)?;

    println!();
    println!("=========================");
    println!("       IMPORTANT!        ");
    println!("=========================");
    println!("These are your wallet seed words.");
    println!("They can be used to recover your wallet and funds.");
    println!("WRITE THEM DOWN OR COPY THEM NOW. THIS IS YOUR ONLY CHANCE TO DO SO.");
    println!();
    println!("=========================");
    println!("{}", seed_words.join(" "));
    println!("=========================");
    println!("\x07"); // beep!

    let mut rl = Editor::<()>::new();
    loop {
        println!("I confirm that I will never see these seed words again.");
        println!(r#"Type the word "confirm" to continue."#);
        let readline = rl.readline(">> ");
        match readline {
            Ok(line) => match line.to_lowercase().as_ref() {
                "confirm" => return Ok(()),
                _ => continue,
            },
            Err(e) => {
                return Err(ExitError::new(ExitCode::IOError, e));
            },
        }
    }
}

/// Clear the terminal and print the Tari splash
pub fn tari_splash_screen(heading: &str) {
    // clear the terminal
    print!("{esc}[2J{esc}[1;1H", esc = 27 as char);

    println!("⠀⠀⠀⠀⠀⣠⣶⣿⣿⣿⣿⣶⣦⣀                                                         ");
    println!("⠀⢀⣤⣾⣿⡿⠋⠀⠀⠀⠀⠉⠛⠿⣿⣿⣶⣤⣀⠀⠀⠀⠀⠀⠀⢰⣿⣾⣾⣾⣾⣾⣾⣾⣾⣾⣿⠀⠀⠀⣾⣾⣾⡀⠀⠀⠀⠀⢰⣾⣾⣾⣾⣿⣶⣶⡀⠀⠀⠀⢸⣾⣿⠀");
    println!("⠀⣿⣿⣿⣿⣿⣶⣶⣤⣄⡀⠀⠀⠀⠀⠀⠉⠛⣿⣿⠀⠀⠀⠀⠀⠈⠉⠉⠉⠉⣿⣿⡏⠉⠉⠉⠉⠀⠀⣰⣿⣿⣿⣿⠀⠀⠀⠀⢸⣿⣿⠉⠉⠉⠛⣿⣿⡆⠀⠀⢸⣿⣿⠀");
    println!("⠀⣿⣿⠀⠀⠀⠈⠙⣿⡿⠿⣿⣿⣿⣶⣶⣤⣤⣿⣿⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣿⣿⡇⠀⠀⠀⠀⠀⢠⣿⣿⠃⣿⣿⣷⠀⠀⠀⢸⣿⣿⣀⣀⣀⣴⣿⣿⠃⠀⠀⢸⣿⣿⠀");
    println!("⠀⣿⣿⣤⠀⠀⠀⢸⣿⡟⠀⠀⠀⠀⠀⠉⣽⣿⣿⠟⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣿⣿⡇⠀⠀⠀⠀⠀⣿⣿⣿⣤⣬⣿⣿⣆⠀⠀⢸⣿⣿⣿⣿⣿⡿⠟⠉⠀⠀⠀⢸⣿⣿⠀");
    println!("⠀⠀⠙⣿⣿⣤⠀⢸⣿⡟⠀⠀⠀⣠⣾⣿⡿⠋⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣿⣿⡇⠀⠀⠀⠀⣾⣿⣿⠿⠿⠿⢿⣿⣿⡀⠀⢸⣿⣿⠙⣿⣿⣿⣄⠀⠀⠀⠀⢸⣿⣿⠀");
    println!("⠀⠀⠀⠀⠙⣿⣿⣼⣿⡟⣀⣶⣿⡿⠋⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣿⣿⡇⠀⠀⠀⣰⣿⣿⠃⠀⠀⠀⠀⣿⣿⣿⠀⢸⣿⣿⠀⠀⠙⣿⣿⣷⣄⠀⠀⢸⣿⣿⠀");
    println!("⠀⠀⠀⠀⠀⠀⠙⣿⣿⣿⣿⠛⠀                                                          ");
    println!("⠀⠀⠀⠀⠀⠀⠀⠀⠙⠁⠀                                                            ");
    println!("{}", heading);
    println!();
}

/// Prompts the user for a new wallet or to recover an existing wallet.
/// Returns the wallet bootmode indicating if it's a new or existing wallet, or if recovery is required.
pub(crate) fn boot(cli: &Cli, wallet_config: &WalletConfig) -> Result<WalletBoot, ExitError> {
    let wallet_exists = wallet_config.db_file.exists();

    // forced recovery
    if cli.recovery {
        if wallet_exists {
            return Err(ExitError::new(
                ExitCode::RecoveryError,
                format!(
                    "Wallet already exists at {:#?}. Remove it if you really want to run recovery in this directory!",
                    wallet_config.db_file
                ),
            ));
        }
        return Ok(WalletBoot::Recovery);
    }

    if wallet_exists {
        // normal startup of existing wallet
        Ok(WalletBoot::Existing)
    } else {
        // automation/wallet created with --password
        if cli.password.is_some() || wallet_config.password.is_some() {
            return Ok(WalletBoot::New);
        }

        // In non-interactive mode, we never prompt. Otherwise, it's not very non-interactive, now is it?
        if cli.non_interactive_mode {
            let msg = "Wallet does not exist and no password was given to create one. Since we're in non-interactive \
                       mode, we need to quit here. Try setting the TARI_WALLET__PASSWORD envar, or setting --password \
                       on the command line";
            return Err(ExitError::new(ExitCode::WalletError, msg));
        }

        // prompt for new or recovery
        let mut rl = Editor::<()>::new();

        loop {
            println!("1. Create a new wallet.");
            println!("2. Recover wallet from seed words.");
            let readline = rl.readline(">> ");
            match readline {
                Ok(line) => {
                    match line.as_ref() {
                        "1" | "c" | "n" | "create" => {
                            // new wallet
                            return Ok(WalletBoot::New);
                        },
                        "2" | "r" | "s" | "recover" => {
                            // recover wallet
                            return Ok(WalletBoot::Recovery);
                        },
                        _ => continue,
                    }
                },
                Err(e) => {
                    return Err(ExitError::new(ExitCode::IOError, e));
                },
            }
        }
    }
}
