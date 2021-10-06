use crate::{format_url, upload_file_to_bucket, Error};
use daedalus::download_file;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub async fn retrieve_data() -> Result<(), Error> {
    let old_manifest =
        daedalus::minecraft::fetch_version_manifest(Some(&*crate::format_url(&*format!(
            "minecraft/v{}/version_manifest.json",
            daedalus::minecraft::CURRENT_FORMAT_VERSION
        ))))
        .await
        .ok();

    let mut manifest = daedalus::minecraft::fetch_version_manifest(None)
        .await?;
    let cloned_manifest = Arc::new(Mutex::new(manifest.clone()));

    let visited_assets_mutex = Arc::new(Mutex::new(Vec::new()));

    let now = Instant::now();

    let mut versions = manifest
        .versions
        .iter_mut()
        .map(|version| async {
            let old_version = if let Some(old_manifest) = &old_manifest {
                old_manifest.versions.iter().find(|x| x.id == version.id)
            } else {
                None
            };

            if let Some(old_version) = old_version {
                if old_version.sha1 == version.sha1 {
                    return Ok(());
                }
            }

            let visited_assets_mutex = Arc::clone(&visited_assets_mutex);
            let cloned_manifest_mutex = Arc::clone(&cloned_manifest);

            let assets_hash = old_version.map(|x| x.assets_index_sha1.clone()).flatten();

            async move {
                let mut upload_futures = Vec::new();

                let now = Instant::now();
                let mut version_println = daedalus::minecraft::fetch_version_info(version)
                    .await
                    ?;
                let elapsed = now.elapsed();
                println!("Version {} Elapsed: {:.2?}", version.id, elapsed);

                let version_path = format!(
                    "minecraft/v{}/versions/{}.json",
                    daedalus::minecraft::CURRENT_FORMAT_VERSION,
                    version.id
                );
                let assets_path = format!(
                    "minecraft/v{}/assets/{}.json",
                    daedalus::minecraft::CURRENT_FORMAT_VERSION,
                    version_println.asset_index.id
                );
                let assets_index_url = version_println.asset_index.url.clone();

                {
                    let mut cloned_manifest = match cloned_manifest_mutex.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => poisoned.into_inner(),
                    };

                    let position = cloned_manifest
                        .versions
                        .iter()
                        .position(|x| version.id == x.id)
                        .unwrap();
                    cloned_manifest.versions[position].url = format_url(&version_path);
                    cloned_manifest.versions[position].assets_index_sha1 =
                        Some(version_println.asset_index.sha1.clone());
                    cloned_manifest.versions[position].assets_index_url =
                        Some(format_url(&assets_path));
                    version_println.asset_index.url = format_url(&assets_path);
                }

                let mut download_assets = false;

                {
                    let mut visited_assets = match visited_assets_mutex.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => poisoned.into_inner(),
                    };

                    if !visited_assets.contains(&version_println.asset_index.id) {
                        if let Some(assets_hash) = assets_hash {
                            if version_println.asset_index.sha1 != assets_hash {
                                download_assets = true;
                            }
                        } else {
                            download_assets = true;
                        }
                    }

                    if download_assets {
                        visited_assets.push(version_println.asset_index.id.clone());
                    }
                }

                if download_assets {
                    let assets_index =
                        download_file(&assets_index_url, Some(&version_println.asset_index.sha1))
                            .await?;

                    {
                        upload_futures
                            .push(upload_file_to_bucket(assets_path, assets_index.to_vec(), Some("application/json".to_string())));
                    }
                }

                {
                    upload_futures.push(upload_file_to_bucket(
                        version_path,
                        serde_json::to_vec(&version_println)?,
                        Some("application/json".to_string())
                    ));
                }

                let now = Instant::now();
                futures::future::try_join_all(upload_futures).await?;
                let elapsed = now.elapsed();
                println!("Spaces Upload {} Elapsed: {:.2?}", version.id, elapsed);

                Ok::<(), Error>(())
            }
            .await?;

            Ok::<(), Error>(())
        })
        .peekable();

    let mut chunk_index = 0;
    while versions.peek().is_some() {
        let now = Instant::now();

        let chunk: Vec<_> = versions.by_ref().take(100).collect();
        futures::future::try_join_all(chunk).await?;

        std::thread::sleep(Duration::from_secs(1));

        chunk_index += 1;

        let elapsed = now.elapsed();
        println!("Chunk {} Elapsed: {:.2?}", chunk_index, elapsed);
    }

    upload_file_to_bucket(
        format!(
            "minecraft/v{}/version_manifest.json",
            daedalus::minecraft::CURRENT_FORMAT_VERSION
        ),
        serde_json::to_vec(&*match cloned_manifest.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        })?,
        Some("application/json".to_string())
    )
    .await?;

    let elapsed = now.elapsed();
    println!("Elapsed: {:.2?}", elapsed);

    Ok(())
}