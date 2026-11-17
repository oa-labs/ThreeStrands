### Improved
- In **Settings → AI Provider**, **Reasoning Model** now sits under **Connection**, just above **Test Connection**. Switching providers clears the old provider's model names, so the new provider's default model is used until you choose another.

### Fixed
- **Test Connection** no longer reports that the model ran out of output space when you use a reasoning model. The test now passes once the provider accepts the endpoint, key, and model.
