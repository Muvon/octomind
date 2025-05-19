// Module for handling embedding generation

use crate::config::{Config, EmbeddingProvider};
use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use fastembed::{TextEmbedding, InitOptions, EmbeddingModel};

// Create a lazy-loaded FastEmbed embedding model cache
lazy_static::lazy_static! {
	static ref CODE_EMBEDDING_MODEL: Arc<Mutex<Option<Arc<TextEmbedding>>>> = Arc::new(Mutex::new(None));
	static ref TEXT_EMBEDDING_MODEL: Arc<Mutex<Option<Arc<TextEmbedding>>>> = Arc::new(Mutex::new(None));
}

// Initialize models on first use
fn get_code_embedding_model(model_name: EmbeddingModel) -> Result<Arc<TextEmbedding>> {
	let mut model_guard = CODE_EMBEDDING_MODEL.lock().unwrap();

	if model_guard.is_none() {
		// Create cache directory if it doesn't exist
		let cache_dir = std::path::PathBuf::from(".octodev/fastembed");
		std::fs::create_dir_all(&cache_dir).context("Failed to create FastEmbed cache directory")?;

		let model = TextEmbedding::try_new(
			InitOptions::new(model_name)
				.with_show_download_progress(true)
				.with_cache_dir(cache_dir),
		)
			.context("Failed to initialize FastEmbed code model")?;

		*model_guard = Some(Arc::new(model));
	}

	// We know it's not None because we just set it if it was
	Ok(model_guard.as_ref().unwrap().clone())
}

fn get_text_embedding_model(model_name: EmbeddingModel) -> Result<Arc<TextEmbedding>> {
	let mut model_guard = TEXT_EMBEDDING_MODEL.lock().unwrap();

	if model_guard.is_none() {
		// Debug output
		// println!("Initializing text embedding model {:?}...", model_name);

		// Create cache directory if it doesn't exist
		let cache_dir = std::path::PathBuf::from(".octodev/fastembed");
		std::fs::create_dir_all(&cache_dir).context("Failed to create FastEmbed cache directory")?;

		let model = TextEmbedding::try_new(
			InitOptions::new(model_name)
				.with_show_download_progress(true)
				.with_cache_dir(cache_dir),
		)
			.context("Failed to initialize FastEmbed text model")?;

		*model_guard = Some(Arc::new(model));
	}

	// We know it's not None because we just set it if it was
	Ok(model_guard.as_ref().unwrap().clone())
}

pub fn calculate_content_hash(contents: &str) -> String {
	let mut hasher = Sha256::new();
	hasher.update(contents.as_bytes());
	format!("{:x}", hasher.finalize())
}

// Calculate a unique hash for a content that includes the file path
pub fn calculate_unique_content_hash(contents: &str, file_path: &str) -> String {
	let mut hasher = Sha256::new();
	hasher.update(contents.as_bytes());
	hasher.update(file_path.as_bytes());
	format!("{:x}", hasher.finalize())
}

// Generate embeddings based on the configured provider
pub async fn generate_embeddings(contents: &str, is_code: bool, config: &Config) -> Result<Vec<f32>> {
	match config.embedding_provider {
		EmbeddingProvider::Jina => {
			let model = if is_code {
				&config.jina.code_model
			} else {
				&config.jina.text_model
			};

			generate_jina_embeddings(contents, model, config).await
		},
		EmbeddingProvider::FastEmbed => {
			let model = if is_code {
				&config.fastembed.code_model
			} else {
				&config.fastembed.text_model
			};

			generate_fastembed_embeddings(contents, model, is_code).await
		}
	}
}

// Generate embeddings with Jina API
async fn generate_jina_embeddings(contents: &str, model: &str, config: &Config) -> Result<Vec<f32>> {
	let result = generate_jina_embeddings_batch(vec![contents.to_string()], model, config).await?;

	match result.first() {
		Some(value) => Ok(value.to_vec()),
		None => Err(anyhow::anyhow!("No embeddings found"))
	}
}

// Generate embeddings with FastEmbed
async fn generate_fastembed_embeddings(contents: &str, model: &str, is_code: bool) -> Result<Vec<f32>> {
	// Use tokio to offload the CPU-intensive embedding generation
	let contents = contents.to_string();
	let model_name = map_model_to_fastembed(model);

	let embedding = tokio::task::spawn_blocking(move || -> Result<Vec<f32>> {
		let model = if is_code {
			get_code_embedding_model(model_name)?
		} else {
			get_text_embedding_model(model_name)?
		};

		let embedding = model.embed(vec![contents], None)?;

		if embedding.is_empty() {
			return Err(anyhow::anyhow!("No embeddings were generated"));
		}

		Ok(embedding[0].clone())
	}).await??;

	Ok(embedding)
}

// Map the text model name to the corresponding FastEmbed model
fn map_model_to_fastembed(model: &str) -> EmbeddingModel {
	match model {
		"all-MiniLM-L6-v2" => EmbeddingModel::AllMiniLML6V2,
		"all-MiniLM-L12-v2" => EmbeddingModel::AllMiniLML12V2,
		"multilingual-e5-small" => EmbeddingModel::MultilingualE5Small,
		"multilingual-e5-base" => EmbeddingModel::MultilingualE5Base,
		"multilingual-e5-large" => EmbeddingModel::MultilingualE5Large,
		_ => panic!("Unsupported embedding model: {}", model),
	}
}

// Generate batch embeddings based on the configured provider
pub async fn generate_embeddings_batch(texts: Vec<String>, is_code: bool, config: &Config) -> Result<Vec<Vec<f32>>> {
	match config.embedding_provider {
		EmbeddingProvider::Jina => {
			let model = if is_code {
				&config.jina.code_model
			} else {
				&config.jina.text_model
			};

			generate_jina_embeddings_batch(texts, model, config).await
		},
		EmbeddingProvider::FastEmbed => {
			let model = if is_code {
				&config.fastembed.code_model
			} else {
				&config.fastembed.text_model
			};

			generate_fastembed_embeddings_batch(texts, model, is_code).await
		}
	}
}

async fn generate_jina_embeddings_batch(texts: Vec<String>, model: &str, config: &Config) -> Result<Vec<Vec<f32>>> {
	let client = Client::new();
	let jina_api_key = match &config.jina_api_key {
		Some(key) => key.clone(),
		None => std::env::var("JINA_API_KEY")
			.context("JINA_API_KEY environment variable not set or not configured in .octodev.toml")?
	};

	let response = client
		.post("https://api.jina.ai/v1/embeddings")
		.header("Authorization", format!("Bearer {}", jina_api_key))
		.json(&json!({
			"input": texts,
			"model": model,
		}))
		.send()
	.await?;

	let response_json: Value = response.json().await?;

	let embeddings = response_json["data"]
		.as_array()
		.context("Failed to get embeddings array")?
		.iter()
		.map(|data| {
			data["embedding"]
				.as_array()
				.unwrap_or(&Vec::new())
				.iter()
				.map(|v| v.as_f64().unwrap_or_default() as f32)
				.collect()
		})
		.collect();

	Ok(embeddings)
}

async fn generate_fastembed_embeddings_batch(texts: Vec<String>, model: &str, is_code: bool) -> Result<Vec<Vec<f32>>> {
	// Use tokio to offload the CPU-intensive embedding generation to a blocking thread
	// let model_name = model.to_string();

	let model_name = map_model_to_fastembed(model);
	let embeddings = tokio::task::spawn_blocking(move || -> Result<Vec<Vec<f32>>> {
		let model = if is_code {
			get_code_embedding_model(model_name)?
		} else {
			get_text_embedding_model(model_name)?
		};

		// Convert Vec<String> to Vec<&str>
		let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();

		let embeddings = model.embed(text_refs, None)?;

		Ok(embeddings)
	}).await??;

	Ok(embeddings)
}
