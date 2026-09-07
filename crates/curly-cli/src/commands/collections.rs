//! `curly collections` — manage collections and their saved requests
//! (FR-5/FR-13). `add-request` intentionally mirrors one-shot mode's flags
//! (see one_shot.rs) so saving a request you just tested with `curly <url>
//! ...` is a matter of re-running the same flags under `collections
//! add-request`, not learning a second flag vocabulary.

use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Subcommand};
use curly_core::extraction::Extraction;
use curly_core::storage::{
    Collection, Folder, KvPair, SavedAuth, SavedBody, SavedMultipartField, SavedRequest, Storage,
};
use reqwest::Method;

use crate::one_shot::{parse_header, parse_kv};

#[derive(Subcommand, Debug)]
pub enum CollectionsCommand {
    /// List all collections
    List,
    /// Create an empty collection
    Create { name: String },
    /// Delete a collection and all its saved requests
    Delete { name: String },
    /// List the requests and folders saved in a collection, as a tree
    Show { name: String },
    /// Save a request into a collection (creating the collection and any
    /// folders in its path if needed) — request_path may be a bare name
    /// ("login") or nested ("Auth/OAuth/login")
    AddRequest {
        collection: String,
        request_path: String,
        #[command(flatten)]
        request: AddRequestArgs,
    },
    /// Remove a saved request from a collection (request_path may be nested)
    RemoveRequest {
        collection: String,
        request_path: String,
    },
    /// Create a folder (and any missing parent folders) inside a collection
    AddFolder {
        collection: String,
        /// May be nested, e.g. "Auth/OAuth"
        folder_path: String,
    },
    /// Remove a folder from a collection
    RemoveFolder {
        collection: String,
        /// May be nested, e.g. "Auth/OAuth"
        folder_path: String,
        /// Required if the folder still has requests or sub-folders in it
        #[arg(long = "force")]
        force: bool,
    },
}

#[derive(Args, Debug)]
pub struct AddRequestArgs {
    /// URL for this saved request (may contain {{variables}})
    pub url: String,

    /// HTTP method (defaults to GET, or POST if a body flag is given)
    #[arg(short = 'X', long = "request")]
    pub method: Option<String>,

    /// Add a request header, e.g. -H "Content-Type: application/json" (repeatable)
    #[arg(short = 'H', long = "header")]
    pub headers: Vec<String>,

    /// Add a query parameter (repeatable)
    #[arg(short = 'Q', long = "query")]
    pub query: Vec<String>,

    /// Save this as the raw request body
    #[arg(short = 'd', long = "data")]
    pub data: Option<String>,

    /// Add an application/x-www-form-urlencoded field (repeatable)
    #[arg(long = "data-urlencode")]
    pub data_urlencode: Vec<String>,

    /// Save a reference to a file whose contents are sent as the raw body
    #[arg(long = "data-binary")]
    pub data_binary: Option<PathBuf>,

    /// Add a multipart/form-data field: "name=value" or "name=@path" (repeatable)
    #[arg(short = 'F', long = "form")]
    pub form: Vec<String>,

    /// Basic auth credentials as "user:password"
    #[arg(short = 'u', long = "user")]
    pub user: Option<String>,

    /// Bearer token
    #[arg(long = "bearer")]
    pub bearer: Option<String>,

    /// Extract a variable from the JSON response body: "NAME=path.in.body" (repeatable)
    #[arg(long = "extract-body")]
    pub extract_body: Vec<String>,

    /// Extract a variable from a response header: "NAME=Header-Name" (repeatable)
    #[arg(long = "extract-header")]
    pub extract_header: Vec<String>,

    /// Compute a variable from a template referencing other variables: "NAME=Bearer {{TOKEN}}" (repeatable)
    #[arg(long = "extract-template")]
    pub extract_template: Vec<String>,

    /// Mask an extracted variable's value in `session show` (name must match one defined by
    /// --extract-body/--extract-header/--extract-template above; repeatable)
    #[arg(long = "extract-secret")]
    pub extract_secret: Vec<String>,
}

fn parse_saved_form_field(raw: &str) -> Result<SavedMultipartField> {
    let (name, value) = parse_kv(raw, "-F/--form")?;
    if let Some(path) = value.strip_prefix('@') {
        Ok(SavedMultipartField::File {
            name,
            path: PathBuf::from(path),
        })
    } else {
        Ok(SavedMultipartField::Text { name, value })
    }
}

fn build_saved_body(args: &AddRequestArgs) -> Result<Option<SavedBody>> {
    let sources_given = [
        args.data.is_some(),
        !args.data_urlencode.is_empty(),
        args.data_binary.is_some(),
        !args.form.is_empty(),
    ]
    .iter()
    .filter(|given| **given)
    .count();

    if sources_given > 1 {
        bail!("only one of -d/--data-urlencode/--data-binary/-F may be used per request");
    }

    if let Some(data) = &args.data {
        return Ok(Some(SavedBody::Raw {
            content: data.clone(),
        }));
    }
    if !args.data_urlencode.is_empty() {
        let fields = args
            .data_urlencode
            .iter()
            .map(|raw| parse_kv(raw, "--data-urlencode"))
            .collect::<Result<Vec<_>>>()?;
        return Ok(Some(SavedBody::Form { fields }));
    }
    if let Some(path) = &args.data_binary {
        return Ok(Some(SavedBody::Binary { path: path.clone() }));
    }
    if !args.form.is_empty() {
        let fields = args
            .form
            .iter()
            .map(|raw| parse_saved_form_field(raw))
            .collect::<Result<Vec<_>>>()?;
        return Ok(Some(SavedBody::Multipart { fields }));
    }

    Ok(None)
}

fn build_saved_auth(args: &AddRequestArgs) -> Result<Option<SavedAuth>> {
    match (&args.user, &args.bearer) {
        (Some(_), Some(_)) => bail!("only one of -u/--user or --bearer may be used per request"),
        (Some(user_pass), None) => {
            let (username, password) = user_pass.split_once(':').ok_or_else(|| {
                anyhow!("invalid -u/--user value (expected \"user:password\"): {user_pass}")
            })?;
            Ok(Some(SavedAuth::Basic {
                username: username.to_string(),
                password: password.to_string(),
            }))
        }
        (None, Some(token)) => Ok(Some(SavedAuth::Bearer {
            token: token.clone(),
        })),
        (None, None) => Ok(None),
    }
}

fn build_extractions(args: &AddRequestArgs) -> Result<Vec<Extraction>> {
    let mut rules = Vec::new();

    for raw in &args.extract_body {
        let (name, path) = parse_kv(raw, "--extract-body")?;
        rules.push(Extraction::Body {
            name,
            path,
            secret: false,
        });
    }
    for raw in &args.extract_header {
        let (name, header) = parse_kv(raw, "--extract-header")?;
        rules.push(Extraction::Header {
            name,
            header,
            secret: false,
        });
    }
    for raw in &args.extract_template {
        let (name, template) = parse_kv(raw, "--extract-template")?;
        rules.push(Extraction::Template {
            name,
            template,
            secret: false,
        });
    }

    for secret_name in &args.extract_secret {
        let rule = rules
            .iter_mut()
            .find(|r| r.name() == secret_name)
            .ok_or_else(|| {
                anyhow!(
                    "--extract-secret {secret_name}: no --extract-body/--extract-header/\
                     --extract-template above defines a variable named \"{secret_name}\""
                )
            })?;
        rule.set_secret(true);
    }

    Ok(rules)
}

fn build_saved_request(name: &str, args: &AddRequestArgs) -> Result<SavedRequest> {
    let body = build_saved_body(args)?;
    let auth = build_saved_auth(args)?;
    let extract = build_extractions(args)?;

    let method = match &args.method {
        Some(m) => Method::from_bytes(m.to_uppercase().as_bytes())
            .with_context(|| format!("invalid HTTP method: {m}"))?,
        None if body.is_some() => Method::POST,
        None => Method::GET,
    };

    let mut saved = SavedRequest::new(name, method, &args.url);

    for raw_header in &args.headers {
        let (n, v) = parse_header(raw_header)?;
        saved.headers.push(KvPair::new(n, v));
    }
    for raw_query in &args.query {
        let (n, v) = parse_kv(raw_query, "-Q/--query")?;
        saved.query_params.push(KvPair::new(n, v));
    }
    saved.body = body;
    saved.auth = auth;
    saved.extract = extract;

    Ok(saved)
}

pub fn run(command: CollectionsCommand, storage: &Storage) -> Result<()> {
    match command {
        CollectionsCommand::List => {
            let names = storage.list_collections()?;
            if names.is_empty() {
                println!("no collections yet — create one with: curly collections create <name>");
            } else {
                for name in names {
                    println!("{name}");
                }
            }
        }

        CollectionsCommand::Create { name } => {
            if storage.load_collection_opt(&name)?.is_some() {
                bail!("collection \"{name}\" already exists");
            }
            storage.save_collection(&Collection::new(&name))?;
            println!("created collection \"{name}\"");
        }

        CollectionsCommand::Delete { name } => {
            storage.delete_collection(&name)?;
            println!("deleted collection \"{name}\"");
        }

        CollectionsCommand::Show { name } => {
            let collection = storage.load_collection(&name)?;
            if collection.folders.is_empty() && collection.requests.is_empty() {
                println!("collection \"{name}\" has no requests yet");
            } else {
                print_tree(&collection.folders, &collection.requests, 0);
            }
        }

        CollectionsCommand::AddRequest {
            collection,
            request_path,
            request,
        } => {
            let mut coll = storage
                .load_collection_opt(&collection)?
                .unwrap_or_else(|| Collection::new(&collection));
            let saved = build_saved_request(&request_path, &request)?;
            coll.add_request(&request_path, saved).with_context(|| {
                format!("in collection \"{collection}\" (remove it first with collections remove-request)")
            })?;
            storage.save_collection(&coll)?;
            println!("saved \"{request_path}\" in collection \"{collection}\"");
        }

        CollectionsCommand::RemoveRequest {
            collection,
            request_path,
        } => {
            let mut coll = storage.load_collection(&collection)?;
            if !coll.remove_request(&request_path) {
                bail!("collection \"{collection}\" has no request named \"{request_path}\"");
            }
            storage.save_collection(&coll)?;
            println!("removed \"{request_path}\" from collection \"{collection}\"");
        }

        CollectionsCommand::AddFolder {
            collection,
            folder_path,
        } => {
            let mut coll = storage
                .load_collection_opt(&collection)?
                .unwrap_or_else(|| Collection::new(&collection));
            coll.add_folder(&folder_path);
            storage.save_collection(&coll)?;
            println!("created folder \"{folder_path}\" in collection \"{collection}\"");
        }

        CollectionsCommand::RemoveFolder {
            collection,
            folder_path,
            force,
        } => {
            let mut coll = storage.load_collection(&collection)?;
            if !coll.remove_folder(&folder_path, force)? {
                bail!("collection \"{collection}\" has no folder named \"{folder_path}\"");
            }
            storage.save_collection(&coll)?;
            println!("removed folder \"{folder_path}\" from collection \"{collection}\"");
        }
    }

    Ok(())
}

fn print_tree(folders: &[Folder], requests: &[SavedRequest], depth: usize) {
    let indent = "  ".repeat(depth);
    for folder in folders {
        println!("{indent}{}/", folder.name);
        print_tree(&folder.folders, &folder.requests, depth + 1);
    }
    for r in requests {
        if r.extract.is_empty() {
            println!("{indent}{:<7} {}", r.method, r.name);
        } else {
            let names: Vec<&str> = r.extract.iter().map(|e| e.name()).collect();
            println!("{indent}{:<7} {}  [extracts: {}]", r.method, r.name, names.join(", "));
        }
    }
}
